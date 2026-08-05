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
use crate::panel::{self, AppState, PanelChrome, PanelSettings, ScrubState};
use crate::panel_settings;
use crate::player::fmt_time_padded;

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
    /// A flexible gap that pushes the pieces around it apart. One per
    /// row under the unique-item model.
    Spacer,
}

/// The row's full catalog in stock order: what the arrange editor offers,
/// and where a menu toggle slots a re-shown piece back in.
const ITEMS: &[panel::ArrangeSpec<SeekItem>] = &[
    panel::ArrangeSpec {
        label: "Elapsed",
        icon: Some(icons::CLOCK),
        value: SeekItem::Elapsed,
    },
    panel::ArrangeSpec {
        label: "Strip",
        icon: Some(icons::AUDIO_LINES),
        value: SeekItem::Strip,
    },
    panel::ArrangeSpec {
        label: "Ending",
        icon: Some(icons::CLOCK),
        value: SeekItem::Ending,
    },
    panel::ArrangeSpec {
        label: "Duration",
        icon: Some(icons::CLOCK),
        value: SeekItem::Duration,
    },
    panel::ArrangeSpec {
        label: "Spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: SeekItem::Spacer,
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
    /// clicking the clock flips it.
    pub show_total: bool,
    /// A thin line at the scrobble threshold, where the playing track
    /// counts as listened for Last.fm. Only draws while scrobbling is
    /// connected and on.
    pub scrobble_marker: bool,
    /// The shown pieces in display order; one not listed is hidden.
    pub items: Vec<SeekItem>,
}

impl Default for SeekConfig {
    fn default() -> Self {
        SeekConfig {
            chrome: PanelChrome::default(),
            show_total: false,
            scrobble_marker: false,
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
    #[serde(default)]
    items: Option<Vec<SeekItem>>,
    #[serde(default = "default_true")]
    timings: bool,
}

impl From<SeekConfigDump> for SeekConfig {
    fn from(dump: SeekConfigDump) -> Self {
        let items = match dump.items {
            Some(items) => panel::dedup(items),
            None if dump.timings => {
                vec![SeekItem::Elapsed, SeekItem::Strip, SeekItem::Ending]
            }
            None => vec![SeekItem::Strip],
        };
        SeekConfig {
            chrome: dump.chrome,
            show_total: dump.show_total,
            scrobble_marker: dump.scrobble_marker,
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
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
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
            focus: cx.focus_handle(),
            tab_panel: None,
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
                        let mut items = this.config.items.clone();
                        for clock in [SeekItem::Elapsed, SeekItem::Ending] {
                            if items.contains(&clock) == timings {
                                items = panel::toggled(ITEMS, &items, clock);
                            }
                        }
                        this.config.items = items;
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
                    "Drag along the bar to reorder; drag between the rows, \
                     or use a chip's x and plus, to hide and show",
                ),
                None,
                panel::arrange_editor(
                    "seek-items",
                    ITEMS,
                    &self.config.items,
                    |this: &mut Self, items, cx| {
                        this.config.items = items;
                        cx.notify();
                    },
                    cx,
                ),
            ))
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

/// The track line centered in whatever height the panel gets: unplayed side
/// dim, played side solid, the waveform's playhead on top. `marker` draws
/// the scrobble threshold as a thin full-height line under the playhead.
fn paint_strip(progress: f32, marker: Option<f32>, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let head_x = progress.clamp(0.0, 1.0) * w;
    let line_y = (h - tokens::SEEK_STRIP_H) / 2.0;
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x, bounds.origin.y + px(line_y)),
            size(px(w), px(tokens::SEEK_STRIP_H)),
        ),
        palette::alpha(palette::accent(), 0x33),
    ));
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.origin.x, bounds.origin.y + px(line_y)),
            size(px(head_x), px(tokens::SEEK_STRIP_H)),
        ),
        palette::accent(),
    ));
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
    window.paint_quad(fill(
        Bounds::new(
            point(
                bounds.origin.x + px(head_x - tokens::PLAYHEAD_W / 2.0),
                bounds.origin.y,
            ),
            size(px(tokens::PLAYHEAD_W), px(h)),
        ),
        palette::alpha(palette::highlight(), 0xd9),
    ));
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
            .items_center();

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
                        paint_strip(progress, marker, bounds, window);
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

        // The row renders the config's list as-is: each shown piece in
        // its place, whatever order the arrange editor left them in. A
        // strip alone keeps running edge to edge; the padding only comes
        // in with a clock.
        let mut track = Some(track);
        let pieces: Vec<AnyElement> = self
            .config
            .items
            .iter()
            .filter_map(|item| match item {
                SeekItem::Elapsed => {
                    Some(clock(fmt_time_padded(now.position_secs, digits)).into_any_element())
                }
                SeekItem::Strip => track.take().map(|t| t.into_any_element()),
                SeekItem::Ending => Some(
                    clock(ending.clone())
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.config.show_total = !this.config.show_total;
                                cx.notify();
                            }),
                        )
                        .into_any_element(),
                ),
                SeekItem::Duration => Some(
                    clock(match now.duration_secs {
                        Some(d) => fmt_time_padded(d, digits),
                        None => "-:--".into(),
                    })
                    .into_any_element(),
                ),
                SeekItem::Spacer => Some(div().flex_1().into_any_element()),
            })
            .collect();

        // Any clock brings the row padding in; the strip alone (spacers
        // included) keeps running edge to edge.
        let has_clock = self
            .config
            .items
            .iter()
            .any(|i| matches!(i, SeekItem::Elapsed | SeekItem::Ending | SeekItem::Duration));
        root.when(has_clock, |d| d.gap(tokens::SPACE_SM).px(tokens::SPACE_SM))
            .children(pieces)
    }
}

// The width is the seek strip's clocks around a usable track.
transport_panel!(SeekStripPanel, "seek", "Seek", min_w = 160.);

#[cfg(test)]
mod tests {
    use super::{SeekConfig, SeekItem};

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

        let saved = serde_json::to_value(&config).unwrap();
        let back: SeekConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }
}
