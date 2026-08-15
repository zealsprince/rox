//! The seek strip panel: a track line with the played side in the accent
//! and a playhead, click or drag to seek, the elapsed and remaining clocks
//! at its ends.

use std::sync::{Arc, LazyLock};

use gpui::{
    canvas, div, fill, point, prelude::*, px, size, AnyElement, App, Bounds, Context, Div,
    EventEmitter, FocusHandle, Focusable, FontFeatures, MouseButton, Pixels, Subscription,
    WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit};
use crate::panel_settings;
use crate::player::fmt_time_padded;
use crate::settings::ui as settings_ui;

use super::{default_true, transport_panel};

/// One piece of the seek row, the arrange editor's unit. The config's
/// list carries the shown ones in display order.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeekItem {
    /// The elapsed clock.
    Elapsed,
    /// The track line itself, click or drag to seek.
    Strip,
    /// The ending clock: time left, or the full duration when toggled.
    Ending,
    /// The full length, always: pairs with the elapsed clock for the
    /// classic "elapsed, total" read without giving up the countdown.
    Duration,
    /// A flexible gap that pushes the pieces around it apart; a row
    /// holds as many as the layout wants.
    Spacer,
    /// A spacer that draws a hairline in the border color across its gap.
    Divider,
    /// The line break: everything after it drops to a second row. The
    /// stacked layouts, where the strip runs the full width with its
    /// clocks over or under it instead of beside.
    Break,
}

/// The two clocks, what the quick Show Timings toggle moves as a pair.
const CLOCKS: [SeekItem; 2] = [SeekItem::Elapsed, SeekItem::Ending];

/// The row's full catalog in stock order: what the arrange editor offers,
/// and where a menu toggle slots a re-shown piece back in.
const ITEMS: &[panel::ArrangeSpec<SeekItem>] = &[
    panel::ArrangeSpec {
        label: "Elapsed",
        icon: Some(icons::CLOCK),
        value: SeekItem::Elapsed,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Strip",
        icon: Some(icons::AUDIO_LINES),
        value: SeekItem::Strip,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Ending",
        icon: Some(icons::CLOCK),
        value: SeekItem::Ending,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Duration",
        icon: Some(icons::CLOCK),
        value: SeekItem::Duration,
        repeats: false,
    },
    panel::ArrangeSpec {
        label: "Spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: SeekItem::Spacer,
        repeats: true,
    },
    panel::ArrangeSpec {
        label: "Divider",
        icon: Some(icons::MINUS),
        value: SeekItem::Divider,
        repeats: true,
    },
];

/// The seek panel's per-view config: what a saved layout restores, and
/// what the panel's dropdown menu edits. Deserialization routes through
/// [`SeekConfigDump`] so layouts from before the row became an ordered
/// list still read.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "SeekConfigDump")]
pub struct SeekConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The ending clock shows the full duration instead of the time left;
    /// the panel settings' Ending row flips it.
    pub show_total: bool,
    /// A thin line at the scrobble threshold, where the playing track
    /// counts as listened for Last.fm. Only draws while scrobbling is
    /// connected and on.
    pub scrobble_marker: bool,
    /// The track line's height in px.
    pub thickness: f32,
    /// The track line's corner radius in px, capped at a pill.
    pub rounding: f32,
    /// The playhead's width in px.
    pub playhead_width: f32,
    /// The playhead spans the strip's full height; off, it hugs the line.
    pub playhead_full: bool,
    /// Cap the full playhead's height in px, kept centered on the line;
    /// 0 lets it fill the panel.
    pub playhead_max: f32,
    /// The shown pieces in display order; one not listed is hidden.
    pub items: Vec<SeekItem>,
}

impl Default for SeekConfig {
    fn default() -> Self {
        SeekConfig {
            chrome: PanelChrome::default(),
            show_total: false,
            scrobble_marker: false,
            thickness: tokens::SEEK_STRIP_H,
            rounding: 0.0,
            playhead_width: tokens::PLAYHEAD_W,
            playhead_full: true,
            playhead_max: 0.0,
            items: vec![SeekItem::Elapsed, SeekItem::Strip, SeekItem::Ending],
        }
    }
}

/// The dump shape [`SeekConfig`] deserializes through: the ordered list
/// newer layouts write, or the retired `timings` toggle that was both
/// clocks around the strip.
#[derive(Deserialize)]
struct SeekConfigDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    show_total: bool,
    #[serde(default)]
    scrobble_marker: bool,
    #[serde(default = "default_thickness")]
    thickness: f32,
    #[serde(default)]
    rounding: f32,
    #[serde(default = "default_playhead_width")]
    playhead_width: f32,
    #[serde(default = "default_true")]
    playhead_full: bool,
    #[serde(default)]
    playhead_max: f32,
    #[serde(default)]
    items: Option<Vec<SeekItem>>,
    #[serde(default = "default_true")]
    timings: bool,
}

fn default_thickness() -> f32 {
    tokens::SEEK_STRIP_H
}

fn default_playhead_width() -> f32 {
    tokens::PLAYHEAD_W
}

impl From<SeekConfigDump> for SeekConfig {
    fn from(dump: SeekConfigDump) -> Self {
        let items = match dump.items {
            // Deduped row by row, the breaks put back after: the catalog
            // doesn't carry the break (it draws as the editor's row
            // boundary, not a chip), and each row may hold its own copy
            // of a piece.
            Some(items) => items
                .split(|i| matches!(i, SeekItem::Break))
                .map(|row| panel::dedup(ITEMS, row.to_vec()))
                .collect::<Vec<_>>()
                .join(&SeekItem::Break),
            None if dump.timings => {
                vec![SeekItem::Elapsed, SeekItem::Strip, SeekItem::Ending]
            }
            None => vec![SeekItem::Strip],
        };
        SeekConfig {
            chrome: dump.chrome,
            show_total: dump.show_total,
            scrobble_marker: dump.scrobble_marker,
            thickness: dump.thickness,
            rounding: dump.rounding,
            playhead_width: dump.playhead_width,
            playhead_full: dump.playhead_full,
            playhead_max: dump.playhead_max,
            items,
        }
    }
}

/// The seek strip: the waveform minus the peaks - a track line with the
/// played side in the accent and a playhead, click or drag to seek, the
/// elapsed and remaining clocks at its ends. Position and seek come off
/// the player the same way the waveform's do.
pub struct SeekStripPanel {
    state: AppState,
    config: SeekConfig,
    /// The strip's painted bounds and drag state, for scrub mapping.
    scrub: ScrubState,
    /// The settings page's scalar strips, with the shared readout edit.
    thickness_scrub: ScrubState,
    rounding_scrub: ScrubState,
    playhead_scrub: ScrubState,
    playhead_max_scrub: ScrubState,
    value_edit: ValueEdit,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The row as it stood when the quick Show Timings toggle last hid the
    /// clocks, so turning them back on returns them to where they sat
    /// rather than their catalog rank. Held on the panel and not the config
    /// because it's the undo for one toggle, not a layout anybody saves.
    timings_stash: Option<Vec<SeekItem>>,
    _player_changed: Subscription,
}

impl SeekStripPanel {
    pub fn new(state: AppState, config: SeekConfig, cx: &mut Context<Self>) -> Self {
        // The clock and the playhead move every tick, so this one wants the
        // raw per-pump notify, not the gated observe the other panels ride.
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        SeekStripPanel {
            state,
            config,
            scrub: ScrubState::default(),
            thickness_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            playhead_scrub: ScrubState::default(),
            playhead_max_scrub: ScrubState::default(),
            value_edit: ValueEdit::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            timings_stash: None,
            _player_changed,
        }
    }

    /// Whether either clock is on the row, what the quick timings toggle
    /// reads and flips.
    fn timings_shown(&self) -> bool {
        self.config
            .items
            .iter()
            .any(|i| matches!(i, SeekItem::Elapsed | SeekItem::Ending))
    }

    /// Both clocks on or off in one move, the row they sat in kept across
    /// the round trip.
    fn toggle_timings(&mut self) {
        self.config.items =
            panel::toggled_stashed(ITEMS, &self.config.items, &mut self.timings_stash, &CLOCKS);
    }

    /// The panel's own dropdown entries: the quick timings and marker
    /// toggles. Timings still means both clocks at once; the settings
    /// window's arrange editor splits and reorders them.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let timings = self.timings_shown();
        let menu = menu.item(
            PopupMenuItem::new("Show Timings")
                .checked(timings)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.toggle_timings();
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Scrobble Marker")
                .checked(self.config.scrobble_marker)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.scrobble_marker = !this.config.scrobble_marker;
                        cx.notify();
                    });
                }),
        )
    }
}

impl PanelSettings for SeekStripPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.config.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.config.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.config.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                "Pieces",
                Some(
                    "Drag along a row to reorder and between rows to move; \
                     a chip's x and plus hide and show",
                ),
                None,
                panel::arrange_rows_editor(
                    "seek-items",
                    ITEMS,
                    &editor_rows(&self.config.items),
                    None,
                    |this: &mut Self, rows, cx| {
                        this.config.items = rows.join(&SeekItem::Break);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Thickness",
                Some("The track line's height"),
                settings_ui::scalar(
                    &self.thickness_scrub,
                    &self.value_edit,
                    self.config.thickness,
                    settings_ui::span(1., 16., " px"),
                    |this: &mut Self, thickness, cx| {
                        this.config.thickness = thickness;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Rounding",
                Some("The line's corner radius, up to a pill at half the thickness"),
                settings_ui::scalar(
                    &self.rounding_scrub,
                    &self.value_edit,
                    self.config.rounding,
                    settings_ui::span(0., 8., " px"),
                    |this: &mut Self, rounding, cx| {
                        this.config.rounding = rounding;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Playhead",
                Some("Span the strip's full height or hug the line"),
                panel::choices(
                    &[("Full", true), ("Line", false)],
                    self.config.playhead_full,
                    |this: &mut Self, full, cx| {
                        this.config.playhead_full = full;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Playhead Width",
                Some("The moving position marker's width"),
                settings_ui::scalar(
                    &self.playhead_scrub,
                    &self.value_edit,
                    self.config.playhead_width,
                    settings_ui::span(1., 8., " px"),
                    |this: &mut Self, width, cx| {
                        this.config.playhead_width = width;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.playhead_full, |d| {
                d.child(panel::setting_row(
                    "Playhead Max Height",
                    Some("Cap the full playhead, centered on the line; 0 fills the panel"),
                    settings_ui::scalar(
                        &self.playhead_max_scrub,
                        &self.value_edit,
                        self.config.playhead_max,
                        settings_ui::span(0., 100., " px"),
                        |this: &mut Self, max, cx| {
                            this.config.playhead_max = max;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .when(self.config.items.contains(&SeekItem::Ending), |d| {
                d.child(panel::setting_row(
                    "Ending",
                    Some("Count down the time left or show the full length"),
                    panel::choices(
                        &[("Remaining", false), ("Total", true)],
                        self.config.show_total,
                        |this: &mut Self, show_total, cx| {
                            this.config.show_total = show_total;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(panel::setting_row(
                "Scrobble Marker",
                Some("A thin line where the track counts as scrobbled to Last.fm"),
                panel::toggle(
                    self.config.scrobble_marker,
                    |this: &mut Self, on, cx| {
                        this.config.scrobble_marker = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

/// The strip's paint knobs, copied off the config for the paint closure.
#[derive(Clone, Copy)]
struct StripLook {
    thickness: f32,
    rounding: f32,
    playhead_width: f32,
    playhead_full: bool,
    playhead_max: f32,
}

impl From<&SeekConfig> for StripLook {
    fn from(config: &SeekConfig) -> Self {
        StripLook {
            thickness: config.thickness,
            rounding: config.rounding,
            playhead_width: config.playhead_width,
            playhead_full: config.playhead_full,
            playhead_max: config.playhead_max,
        }
    }
}

/// The track line centered in whatever height the panel gets: unplayed side
/// dim, played side solid, the waveform's playhead on top. `look` carries
/// the config's line and playhead knobs, the radius capped at a pill.
/// `marker` draws the scrobble threshold as a thin full-height line under
/// the playhead.
fn paint_strip(
    progress: f32,
    marker: Option<f32>,
    look: StripLook,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let head_x = progress.clamp(0.0, 1.0) * w;
    let line_h = look.thickness.clamp(1.0, h);
    let radius = look.rounding.clamp(0.0, line_h / 2.0);
    let line_y = (h - line_h) / 2.0;
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x, bounds.origin.y + px(line_y)),
                size(px(w), px(line_h)),
            ),
            palette::alpha(palette::accent(), 0x33),
        )
        .corner_radii(px(radius)),
    );
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x, bounds.origin.y + px(line_y)),
                size(px(head_x), px(line_h)),
            ),
            palette::accent(),
        )
        // gpui doesn't clamp radii to the quad, so the played side's
        // shrink with it near the start.
        .corner_radii(px(radius.min(head_x / 2.0))),
    );
    if let Some(marker) = marker {
        window.paint_quad(fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(marker.clamp(0.0, 1.0) * w),
                    bounds.origin.y,
                ),
                size(px(1.0), px(h)),
            ),
            palette::alpha(palette::highlight(), 0x80),
        ));
    }
    // The playhead: the panel's full height, capped when the config says
    // so, or the line's when it hugs. Either way it centers on the line.
    let head_w = look.playhead_width.clamp(1.0, w);
    let head_h = if !look.playhead_full {
        line_h
    } else if look.playhead_max > 0.0 {
        look.playhead_max.clamp(line_h.min(h), h)
    } else {
        h
    };
    window.paint_quad(
        fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(head_x - head_w / 2.0),
                    bounds.origin.y + px((h - head_h) / 2.0),
                ),
                size(px(head_w), px(head_h)),
            ),
            palette::alpha(palette::highlight(), 0xd9),
        )
        // The playhead reads the config's rounding raw, capped at its own
        // pill: through the line's cap a head fatter than the line could
        // never close into a circle.
        .corner_radii(px(look.rounding.clamp(0.0, head_w.min(head_h) / 2.0))),
    );
}

/// The config's list cut at the break into one piece list per row. No
/// break reads as the single row the panel has always drawn, and a break
/// with nothing on a side drops the empty row rather than rendering it.
fn split_rows(items: &[SeekItem]) -> Vec<Vec<SeekItem>> {
    items
        .split(|i| matches!(i, SeekItem::Break))
        .filter(|row| !row.is_empty())
        .map(|row| row.to_vec())
        .collect()
}

/// [`split_rows`] for the rows editor, empty rows kept: an added row's
/// well shows until a piece lands or its x drops it.
fn editor_rows(items: &[SeekItem]) -> Vec<Vec<SeekItem>> {
    items
        .split(|i| matches!(i, SeekItem::Break))
        .map(|row| row.to_vec())
        .collect()
}

/// Tabular digits for the clock, built once - [`clock`] runs twice per
/// pump tick while playing, so the feature list should not reallocate
/// every call.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// A clock beside the strip: muted, fixed in the row, digits tabular so a
/// tick never changes the text width.
fn clock(text: String) -> Div {
    let mut clock = div().flex_none().text_color(palette::text_muted());
    clock
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(TNUM.clone());
    clock.child(text)
}

impl Render for SeekStripPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl SeekStripPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let now = self.state.player.read(cx).now_playing();

        // No frame polling: the raw observe in `new` re-renders the strip
        // on every pump tick while audio moves, which is the rate the clock
        // and playhead actually change at. A per-frame request on top only
        // redraws identical pixels - and kept the whole window repainting
        // at refresh rate through a paused session. Scrub drags notify on
        // their own through the mouse handlers.

        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .flex_col()
            .justify_center();

        let Some(now) = now else {
            // Idle: the strip stays blank until a session brings a track.
            return root;
        };

        let progress = now
            .duration_secs
            .filter(|d| *d > 0.0)
            .map(|d| (now.position_secs / d) as f32)
            .unwrap_or(0.0);
        // The marker only shows where a scrobble could actually land: the
        // toggle on and the scrobbler armed.
        let marker = (self.config.scrobble_marker)
            .then(|| self.state.scrobbler.read(cx).marker())
            .flatten();
        // The seek click lives on the track alone so the clocks beside it
        // stay inert.
        // The seek preview shows once the duration resolves; before that a
        // fraction maps to nothing.
        let hover_duration = now.duration_secs.filter(|d| *d > 0.0);
        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let look = StripLook::from(&self.config);
        let track = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.scrub.begin();
                    if let Some(fraction) = this.scrub.fraction(event.position.x) {
                        panel::seek_fraction(&this.state.player, fraction, cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    move |bounds, _, window, _| {
                        paint_strip(progress, marker, look, bounds, window);
                        panel::scrub_on_paint(&scrub, window, {
                            let player = player.clone();
                            move |fraction, cx| panel::seek_fraction(&player, fraction, cx)
                        });
                    },
                )
                .size_full(),
            )
            .when_some(hover_duration, |d, duration| {
                d.child(panel::seek_hover(&self.scrub, duration, cx))
            });

        // The clocks around the strip: the ending one counts down, or
        // shows the full duration when toggled, and "-:--" until the
        // duration resolves. Minutes pad to the duration's digits so
        // neither clock changes width mid-track and wiggles the strip.
        let digits = now
            .duration_secs
            .map(|d| (d as u64 / 60).to_string().len())
            .unwrap_or(1);
        let ending = match now.duration_secs {
            Some(d) if self.config.show_total => fmt_time_padded(d, digits),
            Some(d) => format!(
                "-{}",
                fmt_time_padded((d - now.position_secs).max(0.0), digits)
            ),
            None => "-:--".into(),
        };

        // The config's list draws in order, cut into rows at the break:
        // each shown piece in its place, whatever order the arrange
        // editor left them in. The strip's row takes whatever height the
        // others leave, so a stacked layout keeps the strip broad and its
        // clocks in a thin line over or under it.
        let mut track = Some(track);
        let mut piece = |item: &SeekItem| -> Option<AnyElement> {
            match item {
                SeekItem::Elapsed => {
                    Some(clock(fmt_time_padded(now.position_secs, digits)).into_any_element())
                }
                SeekItem::Strip => track.take().map(|t| t.into_any_element()),
                SeekItem::Ending => Some(clock(ending.clone()).into_any_element()),
                SeekItem::Duration => Some(
                    clock(match now.duration_secs {
                        Some(d) => fmt_time_padded(d, digits),
                        None => "-:--".into(),
                    })
                    .into_any_element(),
                ),
                SeekItem::Spacer => Some(div().flex_1().into_any_element()),
                SeekItem::Divider => Some(
                    div()
                        .flex_1()
                        .h(px(1.))
                        .bg(palette::border())
                        .into_any_element(),
                ),
                SeekItem::Break => None,
            }
        };
        let rows: Vec<Div> = split_rows(&self.config.items)
            .into_iter()
            .map(|items| {
                // Any clock brings its row's padding in; a row of the
                // strip alone (spacers included) runs edge to edge.
                let has_clock = items.iter().any(|i| {
                    matches!(i, SeekItem::Elapsed | SeekItem::Ending | SeekItem::Duration)
                });
                let stretch = items.contains(&SeekItem::Strip);
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .map(|d| {
                        if stretch {
                            d.flex_1().min_h_0()
                        } else {
                            d.flex_none()
                        }
                    })
                    .when(has_clock, |d| d.gap(tokens::SPACE_SM).px(tokens::SPACE_SM))
                    .children(items.iter().filter_map(&mut piece))
            })
            .collect();
        root.children(rows)
    }
}

// The width is the seek strip's clocks around a usable track.
transport_panel!(SeekStripPanel, "seek", "Seek", min_w = 160.);

#[cfg(test)]
mod tests {
    use super::{editor_rows, split_rows, SeekConfig, SeekItem};

    /// A layout with no fields decodes to the stock row, and the retired
    /// timings toggle still reads: off leaves the strip alone.
    #[test]
    fn legacy_timings_folds_into_the_item_list() {
        let config: SeekConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == SeekConfig::default().items);

        let config: SeekConfig = serde_json::from_str(r#"{"timings": false}"#).unwrap();
        assert!(config.items == vec![SeekItem::Strip]);
    }

    /// A layout that carries the list uses it as-is, duplicates dropped,
    /// and round-trips through a save.
    #[test]
    fn item_lists_read_ordered_and_deduped() {
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["strip", "elapsed", "strip"]}"#).unwrap();
        assert!(config.items == vec![SeekItem::Strip, SeekItem::Elapsed]);

        // Uniqueness is per row: a copy on the other side of a break
        // survives the load, only same-row repeats collapse.
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["elapsed", "break", "elapsed"]}"#).unwrap();
        assert!(config.items == vec![SeekItem::Elapsed, SeekItem::Break, SeekItem::Elapsed]);

        let saved = serde_json::to_value(&config).unwrap();
        let back: SeekConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }

    /// A break reads from a layout and cuts the list into rows, with an
    /// empty side dropping its row instead of drawing one.
    #[test]
    fn break_cuts_the_list_into_rows() {
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["strip", "break", "elapsed", "ending"]}"#).unwrap();
        let rows = split_rows(&config.items);
        assert!(
            rows == vec![
                vec![SeekItem::Strip],
                vec![SeekItem::Elapsed, SeekItem::Ending]
            ]
        );

        let rows = split_rows(&[SeekItem::Break, SeekItem::Strip]);
        assert!(rows == vec![vec![SeekItem::Strip]]);

        let rows = split_rows(&SeekConfig::default().items);
        assert!(rows == vec![SeekConfig::default().items]);
    }

    /// The editor's rows keep the empty well a trailing break makes, and
    /// the join puts the breaks back exactly.
    #[test]
    fn editor_rows_keep_empties_and_rejoin() {
        let items = vec![SeekItem::Strip, SeekItem::Break];
        let rows = editor_rows(&items);
        assert!(rows == vec![vec![SeekItem::Strip], vec![]]);
        assert!(rows.join(&SeekItem::Break) == items);
    }
}
