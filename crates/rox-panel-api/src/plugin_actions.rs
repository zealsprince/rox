//! A plugin's declared actions in rox's menus (ADR 30's actions amendment):
//! the menu items, the dialog that asks for an action's params, and the
//! toasts that report how it went. The work runs in the plugin; everything
//! here is rox's own drawing around it.

use std::collections::HashMap;
use std::path::Path;

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Entity, SharedString, Window, div, prelude::*, px, size,
};
use gpui_component::Icon;
use gpui_component::input::{Input, InputState};
use gpui_component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_kit::ui::{select_field, small_button};
use rox_services::plugin_actions::{self, ActionDecl, JobEnd, Outcome, Started};
use serde_json::{Map, Value};

use crate::panel::{AppState, Tone, plugin_item};
use crate::toast::Toast;

/// The actions a plugin declares for its tracks, when every row in `ids` is
/// one plugin's. A selection that spans plugins, or holds local files, has
/// no action all of it can take.
pub fn items(menu: PopupMenu, state: &AppState, ids: &[i64], cx: &mut App) -> PopupMenu {
    let keys = state.library.read(cx).keys_for(ids).unwrap_or_default();
    let rows: Vec<(String, String)> = keys.iter().filter_map(plugin_item).collect();

    let Some((source, _)) = rows.first() else {
        return menu;
    };
    if rows.len() != keys.len() || rows.iter().any(|(s, _)| s != source) {
        return menu;
    }

    let source = source.clone();
    let items = rows.into_iter().map(|(_, key)| key).collect();

    // Library rows carry no flags of their own: these are the newest the
    // session saw for them, from a listing or an action.
    let flags = plugin_actions::known_flags(&source);
    offer(menu, &source, "track", items, &flags)
}

pub fn offers(source: &str, target: &str) -> bool {
    plugin_actions::actions(source)
        .iter()
        .any(|action| action.offered_on(target))
}

/// An entry for every action `source` offers on `target` (`track`, `node`,
/// `source`), each run on `items`. An action with a `when` shows if any item
/// can take it, by its `flags`; an item missing there can take any. The
/// caller sets the plugin's section apart with separators.
pub fn offer(
    menu: PopupMenu,
    source: &str,
    target: &str,
    items: Vec<String>,
    flags: &HashMap<String, Vec<String>>,
) -> PopupMenu {
    plugin_actions::actions(source)
        .into_iter()
        .filter(|action| action.offered_on(target))
        .filter(|action| {
            items.is_empty()
                || items
                    .iter()
                    .any(|item| action.applies_to(flags.get(item).map(Vec::as_slice)))
        })
        .fold(menu, |menu, action| {
            let (source, items) = (source.to_string(), items.clone());
            let icon = rox_services::plugins::action_icon(&source, &action.id)
                .unwrap_or_else(|| icons::PLUG.into());

            menu.item(
                PopupMenuItem::new(action.label.clone())
                    .icon(Icon::default().path(icon))
                    .on_click(move |_, window, cx| {
                        pick(source.clone(), action.clone(), items.clone(), window, cx);
                    }),
            )
        })
}

/// Runs the action, asking for its params first when it declares any.
pub fn pick(
    source: String,
    action: ActionDecl,
    items: Vec<String>,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = window.window_handle();

    if action.param_fields().is_empty() {
        run(source, action, items, Value::Object(Map::new()), origin, cx);
        return;
    }

    let title = SharedString::from(action.label.clone());
    let rows = action.param_fields().len() as f32;
    let bounds = Bounds::centered(None, size(px(420.), px(110. + rows * 64.)), cx);

    crate::panel::open_fixed_window(cx, title, bounds, move |window, cx| {
        cx.new(|cx| ActionForm::new(source, action, items, origin, window, cx))
    });
}

fn run(
    source: String,
    action: ActionDecl,
    items: Vec<String>,
    params: Value,
    origin: AnyWindowHandle,
    cx: &mut App,
) {
    let task = plugin_actions::run(&source, &action, items, params, cx);
    let label = action.label;

    cx.spawn(async move |cx| {
        let started = task.await;

        cx.update(|cx| match started {
            Ok(Started::Done(outcome)) => finished(&label, outcome, true, origin, cx),

            Ok(Started::Job(job)) => {
                crate::openers::task_started(cx);
                watch(job, label, true, origin, cx);
            }

            Err(e) => {
                log::warn!("{source}: action {label}: {e}");
                failed(&label, e, origin, cx);
            }
        })
        .ok();
    })
    .detach();
}

/// What an MCP client's action answered, toasted the way a menu's is so the
/// user sees what was done through the plugin.
pub fn client_ran(label: &str, outcome: Outcome, origin: AnyWindowHandle, cx: &mut App) {
    finished(label, outcome, false, origin, cx);
}

pub fn client_failed(label: &str, error: String, origin: AnyWindowHandle, cx: &mut App) {
    failed(label, error, origin, cx);
}

/// Polls a job an MCP client started until it ends, and toasts the end.
pub fn watch_client_job(
    job: std::sync::Arc<plugin_actions::Job>,
    label: String,
    origin: AnyWindowHandle,
    cx: &mut App,
) {
    crate::openers::task_started(cx);
    watch(job, label, false, origin, cx);
}

/// `asked` is whether the user picked the action here, rather than an MCP
/// client running it.
fn watch(
    job: std::sync::Arc<plugin_actions::Job>,
    label: String,
    asked: bool,
    origin: AnyWindowHandle,
    cx: &mut App,
) {
    let task = plugin_actions::watch(job, cx);

    cx.spawn(async move |cx| {
        let end = task.await;

        cx.update(|cx| match end {
            JobEnd::Finished(outcome) => finished(&label, outcome, asked, origin, cx),
            JobEnd::Failed(e) => failed(&label, e, origin, cx),

            JobEnd::Stopped => {
                Toast::new(
                    Tone::Info,
                    rox_i18n::t!("plugin-action-stopped", action = label),
                )
                .post(origin, cx);
            }
        })
        .ok();
    })
    .detach();
}

/// A result that names a folder or a page keeps its toast up until it's
/// used or dismissed, since the button is the point of it.
fn finished(label: &str, outcome: Outcome, asked: bool, origin: AnyWindowHandle, cx: &mut App) {
    // An answer that's only a path is the action itself, like Show in Folder:
    // the click already asked for it, so it opens without a toast. An MCP
    // client never gets a file manager opened without the user's click.
    if asked
        && outcome.message.is_empty()
        && outcome.link.is_none()
        && let Some(path) = &outcome.reveal
    {
        reveal(path, cx);
        return;
    }

    let message: SharedString = match outcome.message.is_empty() {
        true => rox_i18n::t!("plugin-action-done", action = label.to_string()),
        false => outcome.message.into(),
    };

    let pinned = outcome.reveal.is_some() || outcome.link.is_some();
    let mut toast = Toast::new(Tone::Good, message).pinned(pinned);

    if !asked {
        toast = toast.title(rox_i18n::t!(
            "plugin-action-by-client",
            action = label.to_string()
        ));
    }

    if let Some(path) = outcome.reveal {
        toast = toast.action(
            rox_i18n::t!("plugin-action-show-in-folder"),
            icons::FOLDER,
            move |_, cx| reveal(&path, cx),
        );
    }

    if let Some(url) = outcome.link {
        toast = toast.action(
            rox_i18n::t!("plugin-action-open-link"),
            icons::EXTERNAL_LINK,
            move |_, cx| cx.open_url(&url),
        );
    }

    toast.post(origin, cx);
}

fn failed(label: &str, error: String, origin: AnyWindowHandle, cx: &mut App) {
    Toast::new(Tone::Bad, error)
        .title(rox_i18n::t!(
            "plugin-action-failed",
            action = label.to_string()
        ))
        .post(origin, cx);
}

/// The path is the plugin's; the wire made sure it's absolute. It's only
/// ever shown in the file manager, never opened.
fn reveal(path: &str, cx: &mut App) {
    let path = Path::new(path);

    match path.exists() {
        true => cx.reveal_path(path),
        false => log::warn!("show in folder: {} is gone", path.display()),
    }
}

/// The dialog an action with params opens: one row per param, drawn from
/// the same JSON Schema subset the Plugins page draws config from.
struct ActionForm {
    source: String,
    action: ActionDecl,
    items: Vec<String>,
    origin: AnyWindowHandle,
    fields: Vec<FormField>,
}

struct FormField {
    key: String,
    label: SharedString,
    description: Option<SharedString>,
    required: bool,
    value: FieldValue,
}

enum FieldValue {
    Text {
        input: Entity<InputState>,
        number: Option<Number>,
    },
    Toggle(bool),
    Choice {
        options: Vec<Value>,
        picked: Value,
    },
}

#[derive(Clone, Copy)]
enum Number {
    Integer,
    Float,
}

impl ActionForm {
    fn new(
        source: String,
        action: ActionDecl,
        items: Vec<String>,
        origin: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let required: Vec<String> = action.params["required"]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .filter_map(|k| k.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let fields = action
            .param_fields()
            .into_iter()
            .map(|(key, schema)| {
                let label = schema["title"].as_str().unwrap_or(&key).to_string().into();
                let description = schema["description"].as_str().map(|d| d.to_string().into());
                let value = field_value(&schema, window, cx);

                FormField {
                    required: required.contains(&key),
                    key,
                    label,
                    description,
                    value,
                }
            })
            .collect();

        ActionForm {
            source,
            action,
            items,
            origin,
            fields,
        }
    }

    /// The params as the plugin gets them. None while a required field is
    /// empty or a number doesn't parse.
    fn params(&self, cx: &App) -> Option<Value> {
        let mut params = Map::new();

        for field in &self.fields {
            let value = match &field.value {
                FieldValue::Toggle(on) => Some(Value::Bool(*on)),
                FieldValue::Choice { picked, .. } => (!picked.is_null()).then(|| picked.clone()),

                FieldValue::Text { input, number } => {
                    let text = input.read(cx).value().trim().to_string();

                    match (text.is_empty(), number) {
                        (true, _) => None,
                        (false, None) => Some(Value::String(text)),
                        (false, Some(Number::Integer)) => Some(text.parse::<i64>().ok()?.into()),
                        (false, Some(Number::Float)) => Some(text.parse::<f64>().ok()?.into()),
                    }
                }
            };

            match value {
                Some(value) => {
                    params.insert(field.key.clone(), value);
                }
                None if field.required => return None,
                None => {}
            }
        }

        Some(Value::Object(params))
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(params) = self.params(cx) else {
            return;
        };

        let (source, action, items) =
            (self.source.clone(), self.action.clone(), self.items.clone());
        run(source, action, items, params, self.origin, cx);

        window.remove_window();
    }

    fn control(&self, index: usize, field: &FormField, cx: &mut Context<Self>) -> gpui::AnyElement {
        match &field.value {
            FieldValue::Text { input, .. } => Input::new(input).w(px(220.)).into_any_element(),

            FieldValue::Toggle(on) => crate::panel::toggle(
                *on,
                move |this: &mut Self, on, cx| {
                    if let Some(FieldValue::Toggle(value)) =
                        this.fields.get_mut(index).map(|f| &mut f.value)
                    {
                        *value = on;
                    }
                    cx.notify();
                },
                cx,
            )
            .into_any_element(),

            FieldValue::Choice { options, picked } => {
                let form = cx.entity().downgrade();
                let options = options.clone();
                let current = picked.clone();

                select_field(
                    SharedString::from(format!("action-param-{}", field.key)),
                    plain(picked),
                    picked.is_null(),
                )
                .dropdown_menu(move |mut menu, _, _| {
                    for option in &options {
                        let (form, value) = (form.clone(), option.clone());

                        menu = menu.item(
                            PopupMenuItem::new(plain(option))
                                .checked(*option == current)
                                .on_click(move |_, _, cx| {
                                    form.update(cx, |this, cx| {
                                        if let Some(FieldValue::Choice { picked, .. }) =
                                            this.fields.get_mut(index).map(|f| &mut f.value)
                                        {
                                            *picked = value.clone();
                                        }
                                        cx.notify();
                                    })
                                    .ok();
                                }),
                        );
                    }

                    menu
                })
                .into_any_element()
            }
        }
    }
}

fn field_value(schema: &Value, window: &mut Window, cx: &mut Context<ActionForm>) -> FieldValue {
    let default = &schema["default"];

    if let Some(options) = schema["enum"].as_array() {
        let picked = match options.contains(default) {
            true => default.clone(),
            false => options.first().cloned().unwrap_or(Value::Null),
        };

        return FieldValue::Choice {
            options: options.clone(),
            picked,
        };
    }

    let number = match schema["type"].as_str() {
        Some("boolean") => return FieldValue::Toggle(default.as_bool().unwrap_or(false)),
        Some("integer") => Some(Number::Integer),
        Some("number") => Some(Number::Float),
        _ => None,
    };

    let secret = schema["format"] == "password";
    let text = plain(default);
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .masked(secret)
            .default_value(text)
    });

    // The Run button's state follows what's typed.
    cx.observe(&input, |_, _, cx| cx.notify()).detach();

    FieldValue::Text { input, number }
}

fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl Render for ActionForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.params(cx).is_some();

        let rows: Vec<_> = (0..self.fields.len())
            .map(|index| {
                let field = &self.fields[index];
                let (label, description) = (field.label.clone(), field.description.clone());
                let control = self.control(index, &self.fields[index], cx);

                crate::panel::setting_row(label, description, control)
            })
            .collect();

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(small_button(
                rox_i18n::t!("plugin-action-run"),
                icons::PLAY,
                !ready,
                cx.listener(|this, _, window, cx| this.submit(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("settings-common-cancel"),
                icons::CLOSE,
                false,
                cx.listener(|_, _, window, _| window.remove_window()),
            ));

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(tokens::SPACE_MD)
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_SM)
                    .children(rows),
            )
            .child(footer)
    }
}
