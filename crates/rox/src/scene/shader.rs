//! A shader item (ADR 33, ADR 23): a `PanelShader` on a scene item, run
//! through the same [`PanelSurface`] a panel's surface shader runs through.
//! The pick, the slots, the routes and the approval check are the panel's
//! own, so a shader in a scene never runs unapproved and is reviewed by
//! the same dump walks.
//!
//! A program that can run as the in-scene primitive paints on the item's
//! projected corners. Any other one (it samples the screen, an image or its
//! last frame, or runs more than one pass) paints flat in the item's box,
//! and only its paint can tell which, so the scene asks the item's view
//! every frame. The edits behind the inspector's shader rows are here too;
//! the rows themselves are in [`crate::scene::overlay`].

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, Context, Entity, EntityId, PathPromptOptions, Render, SharedString,
    Window, div, point, prelude::*, px, size,
};
use rox_design::palette::{self, Sides};
use rox_panel_api::panel::shader::{self as surface, PanelSurface};
use rox_panel_api::panel::{PanelChrome, PanelSettings, PanelShader, ScrubState};
use rox_panel_api::panel_settings::ShaderNameField;
use rox_panel_api::signal_ui::routes::RouteEditState;
use rox_scene::{Content, Projected, SceneItem, SceneState};
use serde_json::Value;

use crate::panels::scene::ScenePanel;
use crate::scene::texture::on_box;
use crate::scene::{bbox, edit};

/// Whether an item sits square to the screen. A shader item does when the
/// program its last paint ran could only run as a screen pass; before its
/// first paint it takes perspective.
pub(crate) fn sits_flat(content: &Content, painted_flat: Option<bool>) -> bool {
    match content {
        Content::Shader { .. } => painted_flat == Some(true),
        other => edit::is_flat(other),
    }
}

/// An item's shader as its panel would read it. A body that doesn't parse
/// reads as an empty pick.
pub(crate) fn of(item: &SceneItem) -> Option<PanelShader> {
    match &item.content {
        Content::Shader { shader } => {
            Some(serde_json::from_value(shader.clone()).unwrap_or_default())
        }
        _ => None,
    }
}

/// What a shader that isn't running shows in design mode.
enum Idle {
    Waiting,
    Off,
    Empty,
    Missing(String),
}

impl Idle {
    fn of(shader: &PanelShader) -> Idle {
        if !shader.enabled {
            return Idle::Off;
        }

        match surface::resolve_source(shader.name.as_deref(), &shader.source) {
            Some(source) if !surface::approved(&source) => Idle::Waiting,
            None if shader.name.is_some() => Idle::Missing(shader.name.clone().unwrap_or_default()),
            _ => Idle::Empty,
        }
    }

    fn label(&self) -> SharedString {
        match self {
            Idle::Waiting => rox_i18n::t!("panel-awaiting-approval"),
            Idle::Off => rox_i18n::t!("scene-shader-off"),
            Idle::Empty => rox_i18n::t!("scene-shader-empty"),
            Idle::Missing(name) => rox_i18n::t!("shader-pick-missing", name = name.clone()),
        }
    }
}

/// The live view per shader item, by item id. Each item paints under a view
/// of its own, since the surface keys its program, its compile message and
/// its file watch by the view that paints it.
#[derive(Default)]
pub(crate) struct Shaders {
    views: HashMap<String, Entity<ShaderView>>,
}

impl Shaders {
    fn view(&mut self, id: &str, cx: &mut Context<ScenePanel>) -> Entity<ShaderView> {
        if let Some(view) = self.views.get(id) {
            return view.clone();
        }

        let scene = cx.entity_id();
        let view = cx.new(|_| ShaderView {
            surface: None,
            face: None,
            flat: Rc::default(),
            scene,
        });
        self.views.insert(id.to_string(), view.clone());
        view
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Entity<ShaderView>> {
        self.views.get(id)
    }

    /// What the item's last paint found, None before one.
    pub(crate) fn painted_flat(&self, id: &str, cx: &App) -> Option<bool> {
        self.views.get(id)?.read(cx).flat.get()
    }

    /// Drops the views of items that are gone or no longer shaders.
    pub(crate) fn keep(&mut self, scene: &SceneState) {
        self.views.retain(|id, _| {
            scene
                .find(id)
                .is_some_and(|item| matches!(item.content, Content::Shader { .. }))
        });
    }
}

pub(crate) struct ShaderView {
    surface: Option<Rc<PanelSurface>>,
    face: Option<Projected>,
    flat: Rc<Cell<Option<bool>>>,
    scene: EntityId,
}

impl Render for ShaderView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (Some(face), Some(surface)) = (self.face, self.surface.clone()) else {
            return div().into_any_element();
        };

        let flat = self.flat.clone();
        let scene = self.scene;
        on_box(face.corners, move |corners, window, cx| {
            let me = window.current_view();
            let erred = surface::error(me).is_some();

            let (origin, extent) = bbox(&corners.map(|p| [f32::from(p.x), f32::from(p.y)]));
            let bounds = Bounds {
                origin: point(px(origin[0]), px(origin[1])),
                size: size(px(extent[0]), px(extent[1])),
            };
            let painted = surface.paint_face(corners, bounds, window, cx);

            // The scene lays the item out by what this paint found, and the
            // inspector shows the compile message. gpui drops a notify made
            // while the window draws, so the scene hears about either once
            // the draw is done.
            let moved = painted.is_some() && painted != flat.get();
            if moved || erred != surface::error(me).is_some() {
                if moved {
                    flat.set(painted);
                }
                cx.defer(move |cx| cx.notify(scene));
            }
        })
    }
}

/// The item's element, or None while nothing should show. A shader that
/// isn't approved, is switched off or names nothing renders as off, as it
/// does on a panel; design mode marks where it sits.
pub(crate) fn element(
    item: &SceneItem,
    projected: Projected,
    design: bool,
    shaders: &mut Shaders,
    cx: &mut Context<ScenePanel>,
) -> Option<AnyElement> {
    let shader = of(item)?;
    let view = shaders.view(&item.id, cx);

    // Built every render, as a panel's surface is, so an approval or a pool
    // edit shows on the next frame.
    let chrome = PanelChrome {
        shader: Some(shader.clone()),
        ..PanelChrome::default()
    };
    let running = PanelSurface::build(&chrome, Sides::default());
    let runs = running.is_some();

    view.update(cx, |view, _| {
        view.surface = running.map(Rc::new);
        view.face = runs.then_some(projected);
    });

    if runs {
        return Some(view.into_any_element());
    }

    design.then(|| placeholder(&projected.corners, Idle::of(&shader).label()))
}

fn placeholder(corners: &[[f32; 2]; 4], label: SharedString) -> AnyElement {
    let (origin, extent) = bbox(corners);

    div()
        .absolute()
        .left(px(origin[0]))
        .top(px(origin[1]))
        .w(px(extent[0]))
        .h(px(extent[1]))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_dashed()
        .border_color(palette::text_faint())
        .text_xs()
        .text_color(palette::text_faint())
        .child(label)
        .into_any_element()
}

// ── The inspector's edits ──

/// The shader rows' own state, kept on the inspector.
pub(crate) struct RowState {
    pub name: RefCell<ShaderNameField>,
    pub routes: RefCell<RouteEditState>,
    /// One per slot, sized at the uniform block's width.
    pub slots: Vec<ScrubState>,
}

impl Default for RowState {
    fn default() -> Self {
        RowState {
            name: RefCell::default(),
            routes: RefCell::default(),
            slots: (0..surface::SLOTS).map(|_| ScrubState::default()).collect(),
        }
    }
}

/// Edits item `id`'s shader through the scene's one edit path, so it lands
/// in undo and saves.
pub(crate) fn edit_item(
    this: &mut ScenePanel,
    id: &str,
    key: &'static str,
    edit: impl FnOnce(&mut PanelShader),
    cx: &mut Context<ScenePanel>,
) {
    let Some(mut shader) = this.config.scene.find(id).and_then(of) else {
        return;
    };
    edit(&mut shader);

    let value = serde_json::to_value(&shader).unwrap_or(Value::Null);
    let id = id.to_string();
    this.change_later(
        key,
        move |scene| {
            if let Some(SceneItem {
                content: Content::Shader { shader },
                ..
            }) = scene.find_mut(&id)
            {
                *shader = value;
            }
        },
        cx,
    );
}

/// The rows only show for the selected item, so their edits land there.
pub(crate) fn edit_selected(
    this: &mut ScenePanel,
    key: &'static str,
    edit: impl FnOnce(&mut PanelShader),
    cx: &mut Context<ScenePanel>,
) {
    if let Some(id) = this.selected.clone() {
        edit_item(this, &id, key, edit, cx);
    }
}

/// For edits that move the source: the old compile message was about text
/// that just left.
fn edit_source(
    this: &mut ScenePanel,
    key: &'static str,
    edit: impl FnOnce(&mut PanelShader),
    cx: &mut Context<ScenePanel>,
) {
    if let Some(view) = this.selected.as_deref().and_then(|id| this.shaders.get(id)) {
        surface::note_error(view.entity_id(), None);
    }
    edit_selected(this, key, edit, cx);
}

fn selected_shader(this: &ScenePanel) -> Option<(String, PanelShader)> {
    let id = this.selected.clone()?;
    let shader = this.config.scene.find(&id).and_then(of)?;
    Some((id, shader))
}

fn note(this: &ScenePanel, id: &str, message: String, cx: &mut Context<ScenePanel>) {
    if let Some(view) = this.shaders.get(id) {
        surface::note_error(view.entity_id(), Some(message));
    }
    cx.notify();
}

/// The approval gate's yes, as a panel's page gives it. The bookmark goes:
/// it named a file on the machine that wrote the bundle.
pub(crate) fn approve(this: &mut ScenePanel, cx: &mut Context<ScenePanel>) {
    let Some((_, shader)) = selected_shader(this) else {
        return;
    };
    let Some(source) = surface::resolve_source(shader.name.as_deref(), &shader.source) else {
        return;
    };

    surface::approve(&source);
    edit_selected(
        this,
        "shader.approve",
        |shader| {
            shader.path = None;
            shader.enabled = true;
        },
        cx,
    );
}

pub(crate) fn set_enabled(this: &mut ScenePanel, on: bool, cx: &mut Context<ScenePanel>) {
    edit_selected(this, "shader.enabled", |shader| shader.enabled = on, cx);
}

pub(crate) fn set_idle(this: &mut ScenePanel, on: bool, cx: &mut Context<ScenePanel>) {
    edit_selected(this, "shader.idle", |shader| shader.run_when_idle = on, cx);
}

pub(crate) fn set_manual(
    this: &mut ScenePanel,
    slot: usize,
    value: f32,
    cx: &mut Context<ScenePanel>,
) {
    edit_selected(
        this,
        "shader.manual",
        |shader| surface::set_manual_value(&mut shader.manual, slot, value),
        cx,
    );
}

pub(crate) fn use_example(this: &mut ScenePanel, index: usize, cx: &mut Context<ScenePanel>) {
    let Some(preset) = surface::PRESETS.get(index) else {
        return;
    };

    let source = preset.source.to_string();
    edit_source(
        this,
        "shader.pick",
        move |shader| {
            shader.source = source;
            shader.name = None;
            shader.path = None;
            shader.enabled = true;
        },
        cx,
    );
}

/// Nothing is approved on the way: a workspace shader that came in with a
/// bundle still has to be read before it runs.
pub(crate) fn use_named(this: &mut ScenePanel, name: String, cx: &mut Context<ScenePanel>) {
    edit_source(
        this,
        "shader.pick",
        move |shader| {
            shader.name = Some(name);
            shader.source = String::new();
            shader.path = None;
            shader.enabled = true;
        },
        cx,
    );
}

pub(crate) fn clear(this: &mut ScenePanel, cx: &mut Context<ScenePanel>) {
    edit_source(
        this,
        "shader.pick",
        |shader| {
            shader.name = None;
            shader.source = String::new();
            shader.path = None;
        },
        cx,
    );
}

pub(crate) fn choose_file(
    this: &mut ScenePanel,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    let Some(id) = this.selected.clone() else {
        return;
    };

    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    cx.spawn_in(window, async move |this, cx| {
        let Ok(Ok(Some(mut paths))) = rx.await else {
            return;
        };
        let Some(path) = paths.pop() else {
            return;
        };

        this.update(cx, |this, cx| load_file(this, &id, path, cx))
            .ok();
    })
    .detach();
}

pub(crate) fn reload(this: &mut ScenePanel, cx: &mut Context<ScenePanel>) {
    if let Some((id, shader)) = selected_shader(this)
        && let Some(path) = shader.path
    {
        load_file(this, &id, path, cx);
    }
}

/// Picking a file approves it, since the gate is for sources nobody chose.
fn load_file(this: &mut ScenePanel, id: &str, path: PathBuf, cx: &mut Context<ScenePanel>) {
    match std::fs::read_to_string(&path) {
        Ok(source) => {
            surface::approve(&source);
            if let Some(view) = this.shaders.get(id) {
                surface::note_error(view.entity_id(), None);
            }

            edit_item(
                this,
                id,
                "shader.pick",
                move |shader| {
                    shader.source = source;
                    shader.name = None;
                    shader.path = Some(path);
                    shader.enabled = true;
                },
                cx,
            );
        }

        Err(e) => note(this, id, format!("reading {}: {e}", path.display()), cx),
    }
}

/// What the eject and save rows call the shader with nothing typed.
pub(crate) fn label(item: &SceneItem) -> String {
    if item.name.is_empty() {
        item.id.clone()
    } else {
        item.name.clone()
    }
}

/// Writes the shader out for an outside editor and opens it. A named one
/// ejects through its pool entry, since its edits belong to every surface
/// on the name.
pub(crate) fn eject(this: &mut ScenePanel, cx: &mut Context<ScenePanel>) {
    let Some((id, shader)) = selected_shader(this) else {
        return;
    };
    let name = this.config.scene.find(&id).map(label).unwrap_or_default();

    let ejected = match shader.name.as_deref() {
        Some(pool) => surface::eject_pool_entry(pool),
        None => surface::eject(&surface::eject_name(&name, &shader.source), &shader.source),
    };

    match ejected {
        Ok(path) => {
            if shader.name.is_none() {
                let bookmark = path.clone();
                edit_selected(
                    this,
                    "shader.eject",
                    move |shader| shader.path = Some(bookmark),
                    cx,
                );
            }
            cx.open_with_system(&path);
        }

        Err(e) => {
            let message = rox_i18n::t!("shader-eject-failed", error = e.to_string()).to_string();
            note(this, &id, message, cx);
        }
    }
}

/// Copies the pool's text in and stops using it by name. The same text, so
/// its approval still holds.
pub(crate) fn detach(this: &mut ScenePanel, cx: &mut Context<ScenePanel>) {
    let Some(entry) = selected_shader(this)
        .and_then(|(_, shader)| shader.name)
        .and_then(|name| rox_core::settings::shader_pool_get(&name))
    else {
        return;
    };

    edit_source(
        this,
        "shader.pick",
        move |shader| {
            shader.source = entry.source;
            shader.name = None;
            shader.path = None;
        },
        cx,
    );
}

pub(crate) fn save(this: &mut ScenePanel, name: String, cx: &mut Context<ScenePanel>) {
    let Some((_, shader)) = selected_shader(this) else {
        return;
    };
    let name = name.trim().to_string();
    if name.is_empty() || shader.source.trim().is_empty() {
        return;
    }

    surface::save_to_pool(&name, &shader.source, shader.path.clone());
    edit_selected(
        this,
        "shader.save",
        move |shader| {
            shader.name = Some(name);
            shader.source = String::new();
            shader.path = None;
        },
        cx,
    );
}

/// The in-app editor over what runs: a named shader edits its pool entry,
/// an inline one this item's text.
pub(crate) fn open_editor(
    this: &mut ScenePanel,
    _window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    use surface::edit::{EditKey, ShaderEditTarget};

    let Some((id, shader)) = selected_shader(this) else {
        return;
    };
    let Some(view) = this.shaders.get(&id).map(Entity::entity_id) else {
        return;
    };

    let target = match shader.name.as_deref() {
        Some(name) => ShaderEditTarget::pool(name),
        None => {
            let me = cx.entity().downgrade();
            let item = id.clone();
            let title = this.config.scene.find(&id).map(label).unwrap_or_default();

            Some(ShaderEditTarget {
                key: EditKey::Panel(view),
                title: title.into(),
                source: shader.source.clone(),
                ctx: surface::ProgramCtx::of(None, shader.path.as_deref()),
                path: shader.path.clone(),
                write: Arc::new(move |source, cx| {
                    surface::note_error(view, None);
                    me.update(cx, |this, cx| {
                        edit_item(
                            this,
                            &item,
                            "shader.source",
                            |shader| {
                                shader.source = source;
                                // Applying an edit is asking to see it.
                                shader.enabled = true;
                            },
                            cx,
                        )
                    })
                    .ok();
                }),
            })
        }
    };

    if let Some(target) = target {
        rox_panel_api::openers::shader_editor(PanelSettings::state(this), target, cx);
    }
}

/// Folds a hot reload from the bookmarked file back into the selected
/// item, so the saved scene holds the text on screen. The surface only has
/// the view's id and can't write back itself.
pub(crate) fn absorb_hot(this: &mut ScenePanel, cx: &mut Context<ScenePanel>) {
    let Some((id, shader)) = selected_shader(this) else {
        return;
    };
    let Some(hot) = this
        .shaders
        .get(&id)
        .and_then(|view| surface::hot_source(view.entity_id()))
    else {
        return;
    };

    if shader.name.is_none() && shader.source != hot {
        edit_item(this, &id, "shader.hot", |shader| shader.source = hot, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::UserShaderCaps;

    fn caps() -> UserShaderCaps {
        UserShaderCaps {
            samples_screen: false,
            uses_prev: false,
            multi_pass: false,
            uses_assets: false,
            uses_mask: false,
        }
    }

    #[test]
    fn a_plain_program_takes_perspective_and_a_chain_is_flat() {
        let shader = Content::Shader {
            shader: serde_json::json!({}),
        };
        let painted = |caps: UserShaderCaps| Some(caps.screen_pass_only());

        assert!(!sits_flat(&shader, painted(caps())));
        assert!(!sits_flat(&shader, None), "before its first paint");

        let chains = [
            UserShaderCaps {
                multi_pass: true,
                ..caps()
            },
            UserShaderCaps {
                samples_screen: true,
                ..caps()
            },
            UserShaderCaps {
                uses_prev: true,
                ..caps()
            },
            UserShaderCaps {
                uses_assets: true,
                ..caps()
            },
            UserShaderCaps {
                uses_mask: true,
                ..caps()
            },
        ];
        for chain in chains {
            assert!(sits_flat(&shader, painted(chain)), "{chain:?}");
        }

        // Whatever a paint said, the other kinds keep their own rule.
        let grid = Content::Grid { dock: Value::Null };
        assert!(sits_flat(&grid, Some(false)));
        let image = Content::Image {
            asset: String::new(),
            fit: rox_scene::ImageFit::Contain,
        };
        assert!(!sits_flat(&image, Some(true)));
    }

    #[test]
    fn an_unparsed_shader_reads_as_an_empty_pick() {
        let item: SceneItem = serde_json::from_value(serde_json::json!({
            "id": "s",
            "content": { "kind": "shader", "shader": { "routes": "not a list" } },
        }))
        .expect("parses");

        let shader = of(&item).expect("a shader item");
        assert!(!shader.runnable());
        assert!(matches!(Idle::of(&shader), Idle::Empty));
    }
}
