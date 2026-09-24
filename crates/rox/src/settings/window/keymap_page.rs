//! The Keymap settings page: every chord rox binds, one row per command.
//!
//! Recording needs a keystroke interceptor, which runs ahead of binding
//! resolution: the keys worth binding mostly already do something, so a plain
//! listener would never see them.

use super::*;

use crate::keymap::{self, Command, Group};

impl SettingsWindow {
    pub(super) fn keymap_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        let mut page = PageBody::new();
        for group in Group::ALL {
            let group = *group;
            page = page.section(Section::new(
                q,
                group.icon(),
                group.label(),
                None,
                |mut rows| {
                    for command in keymap::COMMANDS.iter().filter(|c| c.group == group) {
                        rows = self.command_row(command, rows, cx);
                    }
                    rows
                },
            ));
        }
        page.section(Section::new(
            q,
            icons::REFRESH_CW,
            rox_i18n::t!("settings-keymap-section-defaults"),
            None,
            |rows| {
                rows.keyed(
                    "settings-keymap-restore-all",
                    &["reset", "restore", "revert", "keymap"],
                    small_button(
                        rox_i18n::t!("settings-keymap-restore"),
                        icons::REFRESH_CW,
                        self.keymap.is_empty(),
                        cx.listener(|this, _, _, cx| {
                            this.keymap_undo = Some(this.keymap.clone());
                            keymap::reset_all(cx);
                            this.keymap_changed(cx);
                        }),
                    ),
                )
                .keyed(
                    "settings-keymap-undo-last",
                    &["undo", "reset", "restore", "keymap"],
                    small_button(
                        rox_i18n::t!("settings-keymap-undo"),
                        icons::SEEK_BACK,
                        self.keymap_undo.is_none(),
                        cx.listener(|this, _, _, cx| {
                            let Some(map) = this.keymap_undo.take() else {
                                return;
                            };
                            keymap::restore(map, cx);
                            this.keymap_changed(cx);
                        }),
                    ),
                )
            },
        ))
    }

    fn command_row<'a>(
        &self,
        command: &'static Command,
        rows: Rows<'a>,
        cx: &mut Context<Self>,
    ) -> Rows<'a> {
        let chords = keymap::chords(command, &self.keymap);
        // `custom` matches keywords only, so the label, description and chords
        // (typed and printed) go in by hand.
        let mut keywords: Vec<String> = vec![command.label.into(), command.description.into()];
        keywords.extend(chords.iter().map(|chord| chord.to_string()));
        keywords.extend(chords.iter().map(|chord| keymap::display(chord)));
        keywords.push("shortcut".into());
        keywords.push("chord".into());
        keywords.push("binding".into());
        let keywords: Vec<&str> = keywords.iter().map(String::as_str).collect();

        let recording = self.recording == Some(command.id);
        let is_default = keymap::is_default(command, &self.keymap);
        // Per chord, since only one of a row's chords may be shadowed.
        let clashes: Vec<(String, &'static str)> = chords
            .iter()
            .filter_map(|chord| {
                keymap::clash(command, chord, &self.keymap)
                    .map(|other| (keymap::display(chord), other))
            })
            .collect();

        let control = self.chord_control(command, &chords, recording, is_default, cx);
        rows.custom(&keywords, move || {
            let mut row = div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(panel::setting_row(
                    command.label,
                    Some(command.description.into()),
                    control,
                ));
            for (chord, other) in clashes {
                row = row.child(div().text_xs().text_color(palette::text_muted()).child(
                    rox_i18n::t!("settings-keymap-clash", chord = chord, other = other),
                ));
            }
            row.into_any_element()
        })
    }

    fn chord_control(
        &self,
        command: &'static Command,
        chords: &[String],
        recording: bool,
        is_default: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // No wrap: a content-sized slot with a wrapping box lays children out
        // one per line.
        let mut control = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(tokens::SPACE_XS);
        // The chords stay up while recording, so the user sees what's taken.
        if chords.is_empty() {
            control = control.child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child(rox_i18n::t!("settings-keymap-not-bound")),
            );
        }
        for chord in chords {
            control = control.child(self.chord_chip(command, chord, cx));
        }
        if recording {
            return control
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text())
                        .child(rox_i18n::t!("settings-keymap-recording")),
                )
                .child(small_button(
                    rox_i18n::t!("workspace-dialog-cancel"),
                    icons::CLOSE,
                    false,
                    cx.listener(|this, _, _, cx| {
                        this.recording = None;
                        cx.notify();
                    }),
                ))
                .into_any_element();
        }
        control
            .child(icon_button(
                icons::PLUS,
                false,
                cx.listener(move |this, _, _, cx| {
                    this.recording = Some(command.id);
                    cx.notify();
                }),
            ))
            .child(icon_button(
                icons::REFRESH_CW,
                is_default,
                cx.listener(move |this, _, _, cx| {
                    if keymap::is_default(command, &this.keymap) {
                        return;
                    }
                    this.keymap_undo = Some(this.keymap.clone());
                    keymap::reset(command.id, cx);
                    this.keymap_changed(cx);
                }),
            ))
            .into_any_element()
    }

    fn chord_chip(&self, command: &'static Command, chord: &str, cx: &mut Context<Self>) -> Div {
        let held = chord.to_string();
        kbd(keymap::display(chord).into())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .child(
                div()
                    .flex_none()
                    .cursor_pointer()
                    .text_color(palette::text_faint())
                    .hover(|d| d.text_color(palette::text()))
                    .child(
                        svg()
                            .path(icons::CLOSE)
                            .size(px(9.))
                            .text_color(palette::text_faint()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // An edit after a reset outdates the undo snapshot.
                            this.keymap_undo = None;
                            keymap::remove(command.id, &held, cx);
                            this.keymap_changed(cx);
                        }),
                    ),
            )
    }

    pub(super) fn keymap_changed(&mut self, cx: &mut Context<Self>) {
        self.keymap = Settings::load().keymap;
        cx.notify();
    }

    pub(super) fn record_keys(window: &mut Window, cx: &mut Context<Self>) -> gpui::Subscription {
        let this = cx.weak_entity();
        let handle = window.window_handle();
        cx.intercept_keystrokes(move |event, window, cx| {
            // Only this window records, so a chord pressed in the workspace
            // still plays music.
            if window.window_handle() != handle {
                return;
            }
            let Some(this) = this.upgrade() else {
                return;
            };
            if this.read(cx).recording.is_none() {
                return;
            }
            let keystroke = event.keystroke.clone();
            // A lone modifier arrives as its own keystroke; swallowing it would
            // look like recording stopped.
            if matches!(
                keystroke.key.as_str(),
                "control" | "shift" | "alt" | "platform" | "function"
            ) {
                return;
            }
            cx.stop_propagation();
            this.update(cx, |this, cx| {
                let Some(id) = this.recording.take() else {
                    return;
                };
                // Only a bare Escape backs out: Shift+Escape is already a
                // default chord.
                if keystroke.key == "escape" && !keystroke.modifiers.modified() {
                    cx.notify();
                    return;
                }
                this.keymap_undo = None;
                keymap::add(id, keystroke.unparse(), cx);
                this.keymap_changed(cx);
            });
        })
    }
}
