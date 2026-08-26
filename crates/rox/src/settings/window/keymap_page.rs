//! The Keymap settings page: every chord rox binds, one row per command,
//! grouped the way the registry groups them. `impl SettingsWindow`
//! methods in a child module, reaching back into the window's private
//! state, the Workspace page's shape.
//!
//! A row is its chords as keycap chips, each with a way off, plus a
//! button that records another. Recording is the only unusual part: the
//! keys someone wants to bind are mostly keys that already do something,
//! so a plain key listener would never see them - the binding fires
//! first. The window instead holds a keystroke interceptor, which runs
//! ahead of binding resolution, and swallows the press while a row is
//! waiting for one.

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
            "Defaults",
            None,
            |rows| {
                rows.keyed(
                    &["reset", "restore", "revert", "keymap"],
                    "Restore Every Chord",
                    Some(
                        "Put every command back on the keys it ships with, including any this \
                         build no longer has a row for"
                            .into(),
                    ),
                    small_button(
                        "Restore",
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
                    &["undo", "reset", "restore", "keymap"],
                    "Undo the Last Reset",
                    Some("Bring back the chords the last reset threw out, row or all".into()),
                    small_button(
                        "Undo",
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

    /// One command's row: the chips, the record button, the reset, and the
    /// clash note underneath when two commands want the same keys.
    fn command_row<'a>(
        &self,
        command: &'static Command,
        rows: Rows<'a>,
        cx: &mut Context<Self>,
    ) -> Rows<'a> {
        let chords = keymap::chords(command, &self.keymap);
        // The row renders through `custom`, which matches keywords only, so
        // the label and description go in by hand or search never sees them.
        // The chords find the row too, both as typed and as printed, so
        // searching "ctrl-p" and searching "Ctrl+P" both land here.
        let mut keywords: Vec<String> = vec![command.label.into(), command.description.into()];
        keywords.extend(chords.iter().map(|chord| chord.to_string()));
        keywords.extend(chords.iter().map(|chord| keymap::display(chord)));
        keywords.push("shortcut".into());
        keywords.push("chord".into());
        keywords.push("binding".into());
        let keywords: Vec<&str> = keywords.iter().map(String::as_str).collect();

        let recording = self.recording == Some(command.id);
        let is_default = keymap::is_default(command, &self.keymap);
        // A clash is worth saying out loud per chord, since only one of a
        // row's chords may be the shadowed one.
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
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(format!(
                            "{chord} is also {other}; only one of them will fire"
                        )),
                );
            }
            row.into_any_element()
        })
    }

    /// The right-hand side of a command's row.
    fn chord_control(
        &self,
        command: &'static Command,
        chords: &[String],
        recording: bool,
        is_default: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // No wrap here, deliberately: a setting row's control slot is
        // content-sized, and a wrapping box with no definite width lays
        // its children out one per line. A command carries a chord or two
        // plus the two buttons, which fits a line at any window width the
        // settings page opens at.
        let mut control = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(tokens::SPACE_XS);
        // The chords stay up while a row records, because recording is
        // adding to them: what's already bound is what someone needs to
        // see to pick a chord that isn't taken.
        if chords.is_empty() {
            control = control.child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child("Not bound"),
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
                        .child("Press the keys"),
                )
                .child(small_button(
                    "Cancel",
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

    /// One keycap chip with the way to take it off. The × sits inside the
    /// chip rather than beside it, so a row of three chords doesn't read
    /// as six separate controls.
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
                            // An edit on top of a reset outdates the undo
                            // snapshot; undoing here would eat this change.
                            this.keymap_undo = None;
                            keymap::remove(command.id, &held, cx);
                            this.keymap_changed(cx);
                        }),
                    ),
            )
    }

    /// Re-read the file after an edit. The page draws off this mirror
    /// rather than loading settings per render, the way every other page
    /// that reads a setting does.
    pub(super) fn keymap_changed(&mut self, cx: &mut Context<Self>) {
        self.keymap = Settings::load().keymap;
        cx.notify();
    }

    /// The interceptor that catches a keystroke for a recording row. Runs
    /// ahead of binding resolution, which is the only place a key that's
    /// already bound can be seen, and stops the press from reaching what
    /// it's bound to.
    pub(super) fn record_keys(window: &mut Window, cx: &mut Context<Self>) -> gpui::Subscription {
        let this = cx.weak_entity();
        let handle = window.window_handle();
        cx.intercept_keystrokes(move |event, window, cx| {
            // Only this window records, so a chord pressed in the
            // workspace while the page sits open still plays music.
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
            // Tapping a modifier on its own arrives as a keystroke of its
            // own. Nobody means to bind it, and swallowing it would make
            // reaching for Ctrl look like the recording had stopped.
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
                // A bare Escape backs out. Modified, it's a chord like any
                // other - Shift+Escape is already one of the defaults.
                if keystroke.key == "escape" && !keystroke.modifiers.modified() {
                    cx.notify();
                    return;
                }
                // Same as the chip's remove: a fresh recording outdates
                // the undo snapshot.
                this.keymap_undo = None;
                keymap::add(id, keystroke.unparse(), cx);
                this.keymap_changed(cx);
            });
        })
    }
}
