//! The Appearance settings page: theme and language, song theming, ADR 10's
//! transparency pair, window chrome, the frame, fonts, the backdrop visual,
//! and the palette editor, a labeled swatch grid per listing group.
//!
//! The editor works on a copy of the user palette, so the swatches show the
//! base even while a playing track's seed tints the app over it; while song
//! theming is on the editor locks, because the track is driving. Edits apply
//! live through the palette setters and persist to the settings file per
//! change, the volume slider's cadence. Palettes import and export as the
//! settings map's role-to-hex JSON, so a file, the settings entry, and a
//! shared theme are one shape.

use super::*;

impl SettingsWindow {
    /// A picker's change: the role into the working palette, out through
    /// the one setter, into the file. Clearing the hex field reads as
    /// back to the role's default.
    pub(super) fn role_edited(
        &mut self,
        index: usize,
        color: Option<Hsla>,
        picker: &Entity<ColorPickerState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let role = &ROLES[index];
        match color {
            Some(color) => (role.set)(&mut self.base, color.to_rgb()),
            None => {
                let default = (role.get)(&self.side_anchor());
                (role.set)(&mut self.base, default);
                picker.update(cx, |picker, cx| picker.set_value(default, window, cx));
            }
        }
        palette::set(self.base, cx);
        // The palette is live above; the file write goes through the
        // debounce, since a picker drag fires a change per tick like the
        // sliders do.
        self.persist_palette = true;
        self.persist_appearance_soon(cx);
    }

    /// The song-theming switch, the Window menu toggle's twin: through
    /// the palette pipe, which also gates the backdrop layers, and into
    /// the file. The toggle reads the palette static, not a cached field,
    /// so the two entry points never show different states.
    fn set_art_theming(&mut self, on: bool, cx: &mut Context<Self>) {
        palette::set_art_theming(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.art_theming = on);
        cx.notify();
    }

    /// The interface language. Through the settings pipe so every window
    /// repaints in the new locale at once; the pick persists as the
    /// registry id, and None keeps following the OS.
    fn set_language(&mut self, language: Option<String>, cx: &mut Context<Self>) {
        settings::set_language(language.as_deref(), cx);
        self.language = language.clone();
        Settings::update(move |s| s.language = language);
        cx.notify();
    }

    /// The theme pick: which palette side renders, with System following
    /// the OS. Through the settings pipe so the side re-resolves and every
    /// window eases over; render then re-seeds the editor onto that side.
    /// The radio reads the settings static, not a cached field, so this
    /// and the theme toggle panel never show different states.
    fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        settings::set_theme(theme, cx);
        Settings::update(move |s| s.theme = theme);
        cx.notify();
    }

    /// The keep-theme switch: holds the active theme's palette under any
    /// cover. Through the palette pipe so open windows ease over, and into
    /// the file.
    fn set_keep_theme(&mut self, on: bool, cx: &mut Context<Self>) {
        self.keep_theme = on;
        palette::set_keep_theme(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.keep_theme = on);
        cx.notify();
    }

    /// The editing side's designed anchor: what a cleared picker returns
    /// a role to and what Reset returns the whole palette to.
    fn side_anchor(&self) -> Palette {
        match self.editor_mode {
            palette::Mode::Dark => Palette::default(),
            palette::Mode::Light => Palette::light(),
        }
    }

    /// Persist the working palette onto its side of the settings file,
    /// the immediate writers' shared tail (inverse, import, song theme).
    fn persist_palette_now(&self) {
        let mode = self.editor_mode;
        let map = self.base.to_map();
        Settings::update(move |s| *s.palette_map_mut(mode) = map);
    }

    /// Point the editor at the side now rendering after a theme switch:
    /// the working copy and every picker move to that theme's palette.
    /// Runs from render, since every switch path (the toggle here, the OS
    /// flipping under System, a workspace apply) repaints all windows.
    pub(super) fn sync_editor_side(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor_mode == palette::mode() {
            return;
        }
        self.editor_mode = palette::mode();
        self.base = palette::theme_palette(self.editor_mode);
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = (role.get)(&self.base);
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
    }

    /// Catch the Typography slider up to a size this window didn't write:
    /// the zoom shortcuts step the same live value from anywhere in the
    /// app. Runs from render, `sync_editor_side`'s shape, since every step
    /// repaints all windows. The slider's own scrub ends up back on the value
    /// it just wrote, so this only moves for outside writers.
    pub(super) fn sync_font_size(&mut self) {
        self.font_size = palette::app_font_size();
    }

    /// The menubar switch, the Window menu toggle's twin: through the
    /// live static so every workspace window drops or regrows its bar,
    /// and into the file. The toggle reads the static, not a cached
    /// field, so the two entry points never show different states.
    fn set_hide_menubar(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_hide_menubar(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.hide_menubar = on);
        cx.notify();
    }

    /// The design-mode switch, the menubar's Window entry from this side.
    /// Same route as the menubar's: the live flag repaints every window,
    /// and the file keeps it across launches.
    fn set_design_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_design_mode(on, cx);
        Settings::update(move |s| s.design_mode = on);
        cx.notify();
    }

    /// The seams switch: through the live static in the dock crate so
    /// every window's panel dividers repaint, and into the file. The
    /// toggle reads the static, like the menubar's.
    fn set_seams(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_seams(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.seams = on);
        cx.notify();
    }

    /// The decorations switch, the Window menu toggle's twin: flip the
    /// flag, persist, and renegotiate the workspace windows.
    fn set_os_decorations(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_os_decorations(on);
        Settings::update(move |s| s.look.bundle.appearance.os_decorations = on);
        crate::workspace::apply_decorations(cx);
        cx.notify();
    }

    /// The fallback titlebar's style and side. Every open window repaints
    /// rather than just the workspaces: the strip draws in the child
    /// windows too, this one included.
    fn set_chrome_style(&mut self, style: ChromeStyle, cx: &mut Context<Self>) {
        settings::set_chrome_style(style);
        Settings::update(move |s| s.look.bundle.appearance.chrome_style = style);
        crate::workspace::refresh_all_windows(cx);
        cx.notify();
    }

    fn set_chrome_side(&mut self, side: ChromeSide, cx: &mut Context<Self>) {
        settings::set_chrome_side(side);
        Settings::update(move |s| s.look.bundle.appearance.chrome_side = side);
        crate::workspace::refresh_all_windows(cx);
        cx.notify();
    }

    /// The resize-border switch. Same shape as the decorations one above,
    /// and only ever shown on Windows.
    fn set_resize_border(&mut self, on: bool, cx: &mut Context<Self>) {
        settings::set_resize_border(on);
        Settings::update(move |s| s.look.bundle.appearance.resize_border = on);
        crate::workspace::apply_resize_border(cx);
        cx.notify();
    }

    /// The app font: through the live static, so every open window
    /// repaints in the new family, and into the file. None follows the
    /// platform default.
    fn set_app_font(&mut self, font: Option<String>, cx: &mut Context<Self>) {
        settings::set_app_font(font.clone(), cx);
        Settings::update(move |s| s.look.bundle.appearance.app_font = font);
        cx.notify();
    }

    /// The app font size: the strip fraction mapped onto whole px across
    /// the shared range, through the palette pipe so every window's rem
    /// follows the scrub live.
    fn set_font_size(&mut self, value: f32, cx: &mut Context<Self>) {
        self.font_size = value;
        palette::set_app_font_size(self.font_size, cx);
        self.persist_appearance_soon(cx);
        cx.notify();
    }

    fn set_surface(&mut self, value: f32, cx: &mut Context<Self>) {
        self.surface_opacity = value;
        self.scalars_edited(cx);
    }

    fn set_backdrop(&mut self, value: f32, cx: &mut Context<Self>) {
        self.backdrop_strength = value;
        self.scalars_edited(cx);
    }

    /// The backdrop-everywhere switch: live into the palette static the
    /// layer's gate reads, straight into the file since a toggle is one
    /// write, not a scrub.
    fn set_backdrop_windows(&mut self, on: bool, cx: &mut Context<Self>) {
        self.backdrop_all_windows = on;
        palette::set_backdrop_all_windows(on, cx);
        Settings::update(move |s| s.look.bundle.appearance.backdrop_all_windows = on);
        cx.notify();
    }

    /// The Milkdrop backdrop's switch. A toggle is one write, so it goes
    /// straight to the file, and every window is woken because a layer that
    /// parked itself renders nothing that would ask for the next frame.
    fn set_backdrop_visual_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.enabled = on;
        self.backdrop_visual_switched(config, cx);
    }

    fn set_backdrop_visual_rotation(&mut self, choice: BackdropRotation, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        choice.apply(&mut config);
        self.backdrop_visual_switched(config, cx);
    }

    /// The lock. Turning it on notes the preset that's up, so a restart
    /// lands back on it; turning it off forgets it, since an unlocked
    /// backdrop starting on last time's preset would read as stuck.
    fn set_backdrop_visual_locked(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.locked = on;
        config.preset = if on {
            crate::backdrop_visual::current_preset()
        } else {
            None
        };
        self.backdrop_visual_switched(config, cx);
    }

    fn set_backdrop_visual_color(
        &mut self,
        color: settings::MilkdropColor,
        cx: &mut Context<Self>,
    ) {
        let mut config = settings::backdrop_visual();
        config.color = color;
        self.backdrop_visual_switched(config, cx);
    }

    /// Seconds, off the strip's own span.
    fn set_backdrop_visual_duration(&mut self, seconds: f32, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.duration_secs = f64::from(seconds.round());
        self.backdrop_visual_edited(config, cx);
    }

    fn set_backdrop_visual_sensitivity(&mut self, value: f32, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.beat_sensitivity = value;
        self.backdrop_visual_edited(config, cx);
    }

    fn set_backdrop_visual_hard_cuts(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.hard_cuts = on;
        self.backdrop_visual_switched(config, cx);
    }

    fn set_backdrop_visual_fade(&mut self, fade: bool, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.fade = fade;
        self.backdrop_visual_switched(config, cx);
    }

    fn set_backdrop_visual_fade_secs(&mut self, seconds: f32, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.fade_secs = seconds;
        self.backdrop_visual_edited(config, cx);
    }

    fn set_backdrop_visual_fps(&mut self, fps: f32, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.fps = fps.round() as u32;
        self.backdrop_visual_edited(config, cx);
    }

    fn set_backdrop_visual_flip_horizontal(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.flip_horizontal = on;
        self.backdrop_visual_switched(config, cx);
    }

    fn set_backdrop_visual_flip_vertical(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.flip_vertical = on;
        self.backdrop_visual_switched(config, cx);
    }

    /// Star or unstar the preset the backdrop is showing. The list is
    /// app-wide, so this is the same write a panel's star makes; the
    /// backdrop's own paint picks the change up through the generation.
    fn toggle_backdrop_visual_favorite(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = crate::backdrop_visual::current_preset() {
            let on = !settings::is_milkdrop_favorite(&path);
            settings::set_milkdrop_favorite(&path, on);
            cx.notify();
        }
    }

    /// A one-shot change: straight to the files, cache and layers woken.
    /// The live cache is the working copy, so a workspace apply that
    /// swapped the look underneath this window is what the next edit
    /// starts from.
    fn backdrop_visual_switched(
        &mut self,
        config: settings::BackdropVisualConfig,
        cx: &mut Context<Self>,
    ) {
        settings::note_backdrop_visual(config.clone());
        Settings::update(move |s| Self::persist_backdrop_visual(s, config));
        crate::backdrop_visual::wake(cx);
        cx.notify();
    }

    /// The config's two halves to their two files: the look's fields into
    /// the bundle, beside Backdrop Strength, and the machine's into
    /// settings.json. The update writes whichever shard actually moved.
    fn persist_backdrop_visual(s: &mut Settings, config: settings::BackdropVisualConfig) {
        s.look.bundle.appearance.milkdrop = config.look();
        s.backdrop_visual = config;
    }

    fn set_backdrop_visual_strength(&mut self, value: f32, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.strength = value;
        self.backdrop_visual_edited(config, cx);
    }

    /// The render scale, taken as a percentage of the window because that's
    /// what the strip reads.
    fn set_backdrop_visual_scale(&mut self, percent: f32, cx: &mut Context<Self>) {
        let mut config = settings::backdrop_visual();
        config.scale = percent / 100.0;
        self.backdrop_visual_edited(config, cx);
    }

    /// Live into the cache the layers read, file write debounced behind it,
    /// the same deal the appearance scalars make: a slider drag would
    /// otherwise rewrite the whole settings file once per tick.
    fn backdrop_visual_edited(
        &mut self,
        config: settings::BackdropVisualConfig,
        cx: &mut Context<Self>,
    ) {
        settings::note_backdrop_visual(config.clone());
        crate::backdrop_visual::wake(cx);
        self.backdrop_visual_persist_gen += 1;
        let generation = self.backdrop_visual_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.backdrop_visual_persist_gen)
                .unwrap_or(generation);
            if latest == generation {
                Settings::update(move |s| Self::persist_backdrop_visual(s, config));
            }
        })
        .detach();
        cx.notify();
    }

    fn scalars_edited(&mut self, cx: &mut Context<Self>) {
        palette::set_scalars(self.surface_opacity, self.backdrop_strength, cx);
        self.persist_appearance_soon(cx);
        cx.notify();
    }

    /// Persist the appearance scalars, frame, and any pending palette edit
    /// after the current scrub settles. Each slider tick or picker change
    /// would otherwise read, parse, and rewrite the whole settings file (dock
    /// dumps and all); the live statics already hold the value, so only the
    /// file write needs to wait for the last tick.
    fn persist_appearance_soon(&mut self, cx: &mut Context<Self>) {
        self.persist_gen += 1;
        let generation = self.persist_gen;
        let (surface, backdrop, frame) = (self.surface_opacity, self.backdrop_strength, self.frame);
        let palette = self
            .persist_palette
            .then(|| (self.editor_mode, self.base.to_map()));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            // A later tick bumped the generation past this capture, so only
            // the last edit in a burst writes. The palette rereads at fire
            // time so an immediate writer (reset, import) that runs inside
            // the wait isn't undone; the capture only stands in when the
            // window closed before the timer.
            let (latest, palette) = this
                .update(cx, |this, _| {
                    (
                        this.persist_gen,
                        this.persist_palette
                            .then(|| (this.editor_mode, this.base.to_map())),
                    )
                })
                .unwrap_or((generation, palette));
            if latest == generation {
                // The font size comes off the live static rather than a
                // capture: the zoom shortcuts write it from outside this
                // window, and one that runs inside the wait would otherwise
                // get rolled back to whatever the slider last wrote.
                let font_size = palette::app_font_size();
                Settings::update(move |s| {
                    s.look.bundle.appearance.surface_opacity = surface;
                    s.look.bundle.appearance.backdrop_strength = backdrop;
                    s.look.bundle.appearance.frame = frame;
                    s.app_font_size = font_size;
                    if let Some((mode, palette)) = palette {
                        *s.palette_map_mut(mode) = palette;
                    }
                });
            }
        })
        .detach();
    }

    // The app-wide frame setters: whole px in, the new default every
    // panel that sets no override of its own takes. A side of None comes
    // off the linked strip and moves all four together.

    fn set_margin(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        self.frame.margin = self.frame.margin.edited(side, value);
        self.frame_edited(cx);
    }

    fn set_padding(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        self.frame.padding = self.frame.padding.edited(side, value);
        self.frame_edited(cx);
    }

    fn set_rounding(&mut self, value: f32, cx: &mut Context<Self>) {
        self.frame.rounding = value;
        self.frame_edited(cx);
    }

    fn set_border(&mut self, side: Option<Side>, value: f32, cx: &mut Context<Self>) {
        self.frame.border = self.frame.border.edited(side, value);
        self.frame_edited(cx);
    }

    // The link toggles. Splitting only opens the sides up; linking
    // flattens them onto the widest, so nothing on screen disappears.

    fn split_margin(&mut self, split: bool, cx: &mut Context<Self>) {
        self.margin_split = split;
        if !split {
            self.frame.margin = self.frame.margin.linked();
        }
        self.frame_edited(cx);
    }

    fn split_padding(&mut self, split: bool, cx: &mut Context<Self>) {
        self.padding_split = split;
        if !split {
            self.frame.padding = self.frame.padding.linked();
        }
        self.frame_edited(cx);
    }

    fn split_border(&mut self, split: bool, cx: &mut Context<Self>) {
        self.border_split = split;
        if !split {
            self.frame.border = self.frame.border.linked();
        }
        self.frame_edited(cx);
    }

    fn frame_edited(&mut self, cx: &mut Context<Self>) {
        settings::set_app_frame(self.frame, cx);
        self.persist_appearance_soon(cx);
        cx.notify();
    }

    /// One app-frame knob's slider row: the value over its 0 to `max`
    /// range, the px readout alongside. Always set, since these are the
    /// defaults themselves; a panel's own settings are where an override
    /// forks off them. Typed values may run past the strip's top, the
    /// setters accept whatever comes in.
    fn frame_row(
        &self,
        scrub: &ScrubState,
        value: f32,
        max: f32,
        apply: fn(&mut Self, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        settings_ui::scalar(
            scrub,
            &self.value_edit,
            value,
            settings_ui::span(0., max, " px"),
            apply,
            cx,
        )
    }

    /// [`frame_row`](Self::frame_row) for a four-sided knob: the link
    /// toggle and, behind it, one strip or four.
    #[allow(clippy::too_many_arguments)]
    fn frame_sides_row(
        &self,
        scrub: &SidesScrub,
        value: Sides,
        split: bool,
        max: f32,
        on_split: fn(&mut Self, bool, &mut Context<Self>),
        apply: fn(&mut Self, Option<Side>, f32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        settings_ui::sides_control(
            scrub,
            &self.value_edit,
            value,
            split,
            settings_ui::span(0., max, " px"),
            on_split,
            apply,
            cx,
        )
    }

    /// A whole palette into the editor at once: the working copy, every
    /// picker, and the live palette. Persisting is the caller's, because
    /// reset writes an empty map where import writes a full one.
    pub(super) fn apply_palette(
        &mut self,
        palette: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.base = palette;
        for (role, picker) in ROLES.iter().zip(&self.pickers) {
            let color = (role.get)(&self.base);
            picker.update(cx, |picker, cx| picker.set_value(color, window, cx));
        }
        palette::set(self.base, cx);
    }

    /// Back to the editing side's stock palette; the file's map empties
    /// rather than filling with defaults.
    fn reset_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_palette(self.side_anchor(), window, cx);
        // Back off the debounced palette writes too, or a settling picker
        // burst would refill the map this just emptied.
        self.persist_palette = false;
        let mode = self.editor_mode;
        Settings::update(move |s| s.palette_map_mut(mode).clear());
    }

    /// Seed the working palette from the other theme's, flipped across
    /// the designed ladders: editing dark, Inverse From Light Theme pulls
    /// the light side's look dark, and the other way around. The map
    /// persists like any other edit, so the flip is kept across a restart.
    fn inverse_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let other = match self.editor_mode {
            palette::Mode::Dark => palette::Mode::Light,
            palette::Mode::Light => palette::Mode::Dark,
        };
        self.apply_palette(palette::theme_palette(other).inverse(), window, cx);
        self.persist_palette_now();
    }

    /// Bake the song theme into the palette: the colors the playing track
    /// derives become the working palette, then song theming turns off so
    /// they hold. The look a track gave the app is kept as a fixed theme.
    /// The resolved palette is read before theming goes off, since turning
    /// it off retargets the tint back to the base.
    fn apply_song_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let themed = palette::resolved();
        self.set_art_theming(false, cx);
        self.apply_palette(themed, window, cx);
        self.persist_palette_now();
    }

    /// Pick a palette file and load it: the same role-to-hex map the
    /// settings file holds, so exports, settings, and shared themes are
    /// one shape. Unknown roles and bad values fall away silently, a
    /// file that isn't a map at all is ignored.
    fn import_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let Some(map) = std::fs::read_to_string(path)
                .ok()
                .and_then(|json| serde_json::from_str::<BTreeMap<String, String>>(&json).ok())
            else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let anchor = this.side_anchor();
                this.apply_palette(Palette::from_map_over(anchor, &map), window, cx);
                this.persist_palette_now();
            })
            .ok();
        })
        .detach();
    }

    /// Save a palette file, [`Palette::to_map`]'s shape: the working
    /// palette, or the derived one while song theming drives the colors,
    /// so a look a track built can leave as a theme.
    fn export_palette(&mut self, cx: &mut Context<Self>) {
        let map = if palette::art_theming() {
            palette::resolved().to_map()
        } else {
            self.base.to_map()
        };
        let home = dirs::home_dir().unwrap_or_default();
        let rx = cx.prompt_for_new_path(&home, Some("palette.json"));
        cx.spawn(async move |_, _| {
            let Ok(Ok(Some(path))) = rx.await else {
                return;
            };
            if let Ok(json) = serde_json::to_string_pretty(&map) {
                std::fs::write(path, json).ok();
            }
        })
        .detach();
    }

    pub(super) fn appearance_page(
        &self,
        q: &Query,
        columns: usize,
        cx: &mut Context<Self>,
    ) -> PageBody {
        PageBody::new()
            .section(Section::new(
                q,
                icons::MENU,
                rox_i18n::t!("settings-appearance-section-interface"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-language",
                        &["language", "locale", "translation"],
                        panel::language_picker(
                            "app-language",
                            self.language.clone(),
                            Self::set_language,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-design-mode",
                        &["edit", "layout", "rearrange", "lock"],
                        panel::toggle(settings::design_mode(), Self::set_design_mode, cx),
                    )
                    .keyed(
                        "settings-appearance-hide-menubar",
                        &["menu bar", "toolbar", "alt"],
                        panel::toggle(settings::hide_menubar(), Self::set_hide_menubar, cx),
                    )
                    .keyed(
                        "settings-appearance-os-decorations",
                        &["title bar", "chrome", "frameless"],
                        panel::toggle(settings::os_decorations(), Self::set_os_decorations, cx),
                    )
                    // Linux only: it's the one platform where the compositor
                    // can refuse the OS frame outright, so the stand-in strip
                    // these two dress is the only thing that ever draws there.
                    // On Windows and macOS the frame always arrives and the
                    // rows would be knobs on something invisible.
                    .when(cfg!(target_os = "linux"), |rows| {
                        rows.keyed(
                            "settings-appearance-chrome-style",
                            &["title bar", "buttons", "close", "traffic lights"],
                            panel::choices_shared(
                                &[
                                    (
                                        rox_i18n::t!("window-controls-style-icons"),
                                        ChromeStyle::Icons,
                                    ),
                                    (
                                        rox_i18n::t!("window-controls-traffic-lights"),
                                        ChromeStyle::Traffic,
                                    ),
                                ],
                                settings::chrome_style(),
                                Self::set_chrome_style,
                                cx,
                            ),
                        )
                        .keyed(
                            "settings-appearance-chrome-side",
                            &["title bar", "buttons", "align", "left", "right"],
                            panel::choices_shared(
                                &[
                                    (
                                        rox_i18n::t!("settings-appearance-chrome-side-left"),
                                        ChromeSide::Left,
                                    ),
                                    (
                                        rox_i18n::t!("settings-appearance-chrome-side-right"),
                                        ChromeSide::Right,
                                    ),
                                ],
                                settings::chrome_side(),
                                Self::set_chrome_side,
                                cx,
                            ),
                        )
                    })
                    // Windows only: on Linux and macOS the borderless window has
                    // no edge resize to take away, so the row would be a switch
                    // that does nothing.
                    .when(cfg!(target_os = "windows"), |rows| {
                        rows.keyed(
                            "settings-appearance-resize-border",
                            &["border", "edge", "frame", "borderless"],
                            panel::toggle(settings::resize_border(), Self::set_resize_border, cx),
                        )
                    })
                },
            ))
            .section(Section::new(
                q,
                icons::CONTRAST,
                rox_i18n::t!("settings-appearance-section-theming"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-theme",
                        &["dark", "light", "mode", "appearance"],
                        panel::choices_shared(
                            &[
                                (rox_i18n::t!("settings-appearance-theme-dark"), Theme::Dark),
                                (
                                    rox_i18n::t!("settings-appearance-theme-light"),
                                    Theme::Light,
                                ),
                                (
                                    rox_i18n::t!("settings-appearance-theme-system"),
                                    Theme::System,
                                ),
                            ],
                            settings::theme(),
                            Self::set_theme,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-song-theming",
                        &["album art", "tint", "accent"],
                        panel::toggle(palette::art_theming(), Self::set_art_theming, cx),
                    )
                    .keyed(
                        "settings-appearance-keep-theme",
                        &["dark", "light", "lock", "pin"],
                        panel::toggle(self.keep_theme, Self::set_keep_theme, cx),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::ALIGN_LEFT,
                rox_i18n::t!("settings-appearance-section-typography"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-font",
                        &["typeface", "family", "text"],
                        panel::font_picker(
                            "app-font",
                            settings::app_font().map(|font| font.to_string()),
                            Self::set_app_font,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-font-size",
                        &["text size", "scale", "zoom"],
                        settings_ui::scalar(
                            &self.font_size_scrub,
                            &self.value_edit,
                            self.font_size,
                            settings_ui::span(
                                palette::FONT_SIZE_MIN,
                                palette::FONT_SIZE_MAX,
                                " px",
                            )
                            .hard(),
                            Self::set_font_size,
                            cx,
                        ),
                    )
                },
            ))
            .section(Section::new(
                q,
                icons::EYE,
                rox_i18n::t!("settings-appearance-section-transparency"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-surface-opacity",
                        &["transparency", "translucent", "blur"],
                        settings_ui::slider_edit(
                            &self.surface_scrub,
                            &self.value_edit,
                            self.surface_opacity,
                            Self::set_surface,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-backdrop-strength",
                        &["transparency", "opacity", "blur", "wallpaper"],
                        settings_ui::slider_edit(
                            &self.backdrop_scrub,
                            &self.value_edit,
                            self.backdrop_strength,
                            Self::set_backdrop,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-backdrop-all-windows",
                        &["transparency", "backdrop", "child windows", "everywhere"],
                        panel::toggle(self.backdrop_all_windows, Self::set_backdrop_windows, cx),
                    )
                },
            ))
            .section(self.backdrop_visual_section(q, cx))
            .section(Section::new(
                q,
                icons::SQUARE_DASHED,
                rox_i18n::t!("settings-appearance-section-frame"),
                None,
                |rows| {
                    rows.keyed(
                        "settings-appearance-margin",
                        &["spacing", "gap", "outside", "sides"],
                        self.frame_sides_row(
                            &self.margin_scrub,
                            self.frame.margin,
                            self.margin_split,
                            MARGIN_MAX,
                            Self::split_margin,
                            Self::set_margin,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-padding",
                        &["spacing", "inset", "inside", "sides"],
                        self.frame_sides_row(
                            &self.padding_scrub,
                            self.frame.padding,
                            self.padding_split,
                            PADDING_MAX,
                            Self::split_padding,
                            Self::set_padding,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-rounding",
                        &["corner radius", "rounded"],
                        self.frame_row(
                            &self.rounding_scrub,
                            self.frame.rounding,
                            ROUNDING_MAX,
                            Self::set_rounding,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-border",
                        &["outline", "stroke", "edge", "sides"],
                        self.frame_sides_row(
                            &self.border_scrub,
                            self.frame.border,
                            self.border_split,
                            BORDER_MAX,
                            Self::split_border,
                            Self::set_border,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-panel-seams",
                        &["divider", "gutter", "grid lines"],
                        panel::toggle(settings::seams(), Self::set_seams, cx),
                    )
                },
            ))
            .section(self.colors_section(q, columns, cx))
    }

    /// The Milkdrop Backdrop section: the frame behind the whole app, drawn
    /// over the blurred cover and under everything else.
    ///
    /// Under Appearance, right after the Transparency section whose
    /// backdrop strength it composites with, because that's where someone
    /// deciding what the app looks like behind its panels is already
    /// looking. It started life at the bottom of the Shader page on the
    /// argument that it's a renderer with a cost, and nobody found it there.
    fn backdrop_visual_section(&self, q: &Query, cx: &mut Context<Self>) -> Section {
        let config = settings::backdrop_visual();
        let error = crate::backdrop_visual::error();
        let current = crate::backdrop_visual::current_preset();
        let has_current = current.is_some();
        let starred = current
            .as_deref()
            .is_some_and(settings::is_milkdrop_favorite);
        let showing: SharedString = current
            .as_deref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().into_owned().into())
            .unwrap_or_else(|| rox_i18n::t!("milkdrop-no-preset"));
        let favorites = settings::milkdrop_favorites().len();
        Section::new(
            q,
            icons::AUDIO_LINES,
            rox_i18n::t!("settings-appearance-section-milkdrop"),
            None,
            move |mut rows| {
                rows = rows.keyed(
                    "settings-appearance-milkdrop-enabled",
                    &["milkdrop", "visual", "visualizer", "background"],
                    panel::toggle(config.enabled, Self::set_backdrop_visual_enabled, cx),
                );
                // Everything under the switch is about a visual that's
                // running; with it off the rows would only be furniture.
                if !config.enabled {
                    return rows;
                }
                rows = rows
                    // The picker works with nothing playing: the worker
                    // takes a load while parked, and one that hasn't
                    // started yet gets the pick at start. Random is
                    // chosen here for the same reason. The row reads the
                    // preset that's up and opens the picker window over
                    // the backdrop; the list itself is too big for a row.
                    .keyed(
                        "settings-appearance-milkdrop-preset",
                        &[
                            "milkdrop",
                            "preset",
                            "now showing",
                            "pick",
                            "search",
                            "choose",
                        ],
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(showing),
                            )
                            .child(small_button(
                                rox_i18n::t!("milkdrop-choose"),
                                icons::SEARCH,
                                false,
                                |_, _, cx| {
                                    crate::milkdrop_picker::open(
                                        Box::new(crate::backdrop_visual::BackdropHost),
                                        cx,
                                    )
                                },
                            )),
                    )
                    .custom(
                        &["milkdrop", "preset", "random", "favorite", "reveal"],
                        || {
                            div()
                                .flex()
                                .flex_row()
                                .flex_wrap()
                                .gap(tokens::SPACE_XS)
                                .child(small_button(
                                    rox_i18n::t!("milkdrop-random"),
                                    icons::SHUFFLE,
                                    false,
                                    |_, _, cx| crate::backdrop_visual::random_preset(cx),
                                ))
                                .child(small_button(
                                    rox_i18n::t!(if starred {
                                        "milkdrop-favorited"
                                    } else {
                                        "milkdrop-favorite-short"
                                    }),
                                    if starred {
                                        icons::STAR_FILLED
                                    } else {
                                        icons::STAR
                                    },
                                    !has_current,
                                    cx.listener(|this, _, _, cx| {
                                        this.toggle_backdrop_visual_favorite(cx)
                                    }),
                                ))
                                .child(small_button(
                                    rox_i18n::t!("milkdrop-reveal-short"),
                                    icons::EXTERNAL_LINK,
                                    !has_current,
                                    move |_, _, cx| {
                                        if let Some(path) = current.as_deref() {
                                            cx.reveal_path(path);
                                        }
                                    },
                                ))
                                .into_any_element()
                        },
                    )
                    .keyed(
                        "settings-appearance-milkdrop-locked",
                        &["milkdrop", "preset", "lock", "hold", "stay"],
                        panel::toggle(config.locked, Self::set_backdrop_visual_locked, cx),
                    )
                    .keyed(
                        "settings-appearance-milkdrop-duration",
                        &["milkdrop", "preset", "seconds", "switch", "rotation"],
                        settings_ui::scalar(
                            &self.backdrop_visual_duration_scrub,
                            &self.value_edit,
                            config.duration_secs as f32,
                            settings_ui::span(1.0, 120.0, " s").hard(),
                            Self::set_backdrop_visual_duration,
                            cx,
                        ),
                    )
                    // The rotation options: everything, the favorites,
                    // then every folder in the scan, the panel's list.
                    .keyed(
                        "settings-appearance-milkdrop-rotation",
                        &[
                            "milkdrop",
                            "favorites",
                            "starred",
                            "shuffle",
                            "folder",
                            "rotation",
                        ],
                        {
                            let mut rotations: Vec<(BackdropRotation, SharedString)> = vec![
                                (BackdropRotation::All, rox_i18n::t!("milkdrop-rotation-all")),
                                (
                                    BackdropRotation::Favorites,
                                    rox_i18n::t!(
                                        "milkdrop-rotation-favorites",
                                        count = favorites.to_string()
                                    ),
                                ),
                            ];
                            rotations.extend(
                                crate::backdrop_visual::rotation_folders().iter().map(
                                    |(key, label)| {
                                        (BackdropRotation::Folder(key.clone()), label.clone())
                                    },
                                ),
                            );
                            panel::picker(
                                "milkdrop-backdrop-rotation",
                                BackdropRotation::of(&config),
                                rotations,
                                false,
                                Self::set_backdrop_visual_rotation,
                                cx,
                            )
                        },
                    )
                    // Favorites picked with nothing starred is the one pick
                    // that does something other than what it says.
                    .when(config.favorites_only && favorites == 0, |rows| {
                        rows.custom(&["milkdrop", "favorites"], || {
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(rox_i18n::t!("milkdrop-no-favorites"))
                                .into_any_element()
                        })
                    })
                    // From here the rows are the panel's Tuning page in
                    // the panel's order, so the two visuals tune alike.
                    .keyed(
                        "settings-appearance-milkdrop-beat-sensitivity",
                        &["milkdrop", "beat", "sensitivity", "detect"],
                        settings_ui::scalar(
                            &self.backdrop_visual_sensitivity_scrub,
                            &self.value_edit,
                            config.beat_sensitivity,
                            settings_ui::span(0.0, 5.0, "").decimals(2).hard(),
                            Self::set_backdrop_visual_sensitivity,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-milkdrop-hard-cuts",
                        &["milkdrop", "beat", "cut", "switch"],
                        panel::toggle(config.hard_cuts, Self::set_backdrop_visual_hard_cuts, cx),
                    )
                    .keyed(
                        "settings-appearance-milkdrop-fps",
                        &["milkdrop", "visual", "frame rate", "performance", "cost"],
                        settings_ui::scalar(
                            &self.backdrop_visual_fps_scrub,
                            &self.value_edit,
                            config.fps as f32,
                            settings_ui::span(
                                settings::BACKDROP_VISUAL_FPS_MIN as f32,
                                settings::BACKDROP_VISUAL_FPS_MAX as f32,
                                " fps",
                            )
                            .hard(),
                            Self::set_backdrop_visual_fps,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-milkdrop-scale",
                        &["milkdrop", "visual", "resolution", "performance", "cost"],
                        settings_ui::scalar(
                            &self.backdrop_visual_scale_scrub,
                            &self.value_edit,
                            config.scale * 100.0,
                            settings_ui::span(10.0, 100.0, "%").hard(),
                            Self::set_backdrop_visual_scale,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-milkdrop-strength",
                        &["milkdrop", "visual", "opacity", "blend", "intensity"],
                        settings_ui::slider_edit(
                            &self.backdrop_visual_strength_scrub,
                            &self.value_edit,
                            config.strength,
                            Self::set_backdrop_visual_strength,
                            cx,
                        ),
                    )
                    // Hold or fade with the audio gone, the panel's switch
                    // with the panel's option labels. The fade time is only
                    // a question once there's a fade to time.
                    .keyed(
                        "settings-appearance-milkdrop-idle",
                        &["milkdrop", "pause", "stop", "hold", "fade", "freeze"],
                        panel::choices_shared(
                            &[
                                (rox_i18n::t!("milkdrop-idle-hold"), false),
                                (rox_i18n::t!("milkdrop-idle-fade"), true),
                            ],
                            config.fade,
                            Self::set_backdrop_visual_fade,
                            cx,
                        ),
                    )
                    .when(config.fade, |rows| {
                        rows.keyed(
                            "settings-appearance-milkdrop-fade-duration",
                            &["milkdrop", "fade", "seconds", "pause", "stop"],
                            settings_ui::scalar(
                                &self.backdrop_visual_fade_scrub,
                                &self.value_edit,
                                config.fade_secs,
                                settings_ui::span(0.0, settings::BACKDROP_VISUAL_FADE_MAX, " s")
                                    .decimals(1)
                                    .hard(),
                                Self::set_backdrop_visual_fade_secs,
                                cx,
                            ),
                        )
                    })
                    .keyed(
                        "settings-appearance-milkdrop-color",
                        &["milkdrop", "light", "dark", "theme", "palette", "invert"],
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("milkdrop-color-preset"),
                                    settings::MilkdropColor::Preset,
                                ),
                                (
                                    rox_i18n::t!("milkdrop-color-theme"),
                                    settings::MilkdropColor::Theme,
                                ),
                                (
                                    rox_i18n::t!("milkdrop-color-palette"),
                                    settings::MilkdropColor::Palette,
                                ),
                                (
                                    rox_i18n::t!("milkdrop-color-cover"),
                                    settings::MilkdropColor::Cover,
                                ),
                            ],
                            config.color,
                            Self::set_backdrop_visual_color,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-milkdrop-flip-horizontal",
                        &["milkdrop", "flip", "mirror", "horizontal"],
                        panel::toggle(
                            config.flip_horizontal,
                            Self::set_backdrop_visual_flip_horizontal,
                            cx,
                        ),
                    )
                    .keyed(
                        "settings-appearance-milkdrop-flip-vertical",
                        &["milkdrop", "flip", "mirror", "vertical"],
                        panel::toggle(
                            config.flip_vertical,
                            Self::set_backdrop_visual_flip_vertical,
                            cx,
                        ),
                    );
                match error {
                    Some(error) => rows.custom(&["milkdrop", "visual", "error"], || {
                        match panel::shader::unsupported(&error) {
                            true => panel::banner(
                                panel::Tone::Bad,
                                panel::shader::NO_PIPELINE_TITLE,
                                vec![panel::shader::NO_PIPELINE_NOTE.into()],
                            ),
                            false => panel::banner(
                                panel::Tone::Bad,
                                rox_i18n::t!("settings-appearance-milkdrop-error-title"),
                                vec![error.into()],
                            ),
                        }
                        .into_any_element()
                    }),
                    None => rows,
                }
            },
        )
    }

    /// One cell of the color grid: the picker with its label beside it,
    /// or a dimmed inert swatch while song theming drives the palette.
    /// The inert swatch shows the derived color the track produced, the
    /// same values export saves, not the base underneath.
    fn color_cell(&self, role: &Role, picker: &Entity<ColorPickerState>, locked: bool) -> Div {
        let control: AnyElement = if locked {
            div()
                .size_5()
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(palette::border())
                .bg((role.get)(&palette::resolved()))
                .opacity(0.5)
                .into_any_element()
        } else {
            // The picker pads a 4px margin around its swatch square; the
            // counter-margin keeps the live cell the same 20px footprint
            // as the locked one, so the grid doesn't loosen when editable.
            ColorPicker::new(picker)
                .small()
                .m(px(-4.))
                .into_any_element()
        };
        settings_ui::color_cell(control, role.label, false, None)
    }

    fn colors_section(&self, q: &Query, columns: usize, cx: &mut Context<Self>) -> Section {
        let locked = palette::art_theming();

        // Import, inverse, and reset lock with the rest of the editor:
        // they change the palette too. Apply Song Theme is the opposite,
        // live only while theming drives the colors it bakes in. Export
        // stays live; unlocked it saves the base palette, locked the
        // derived one the swatches show.
        let inverse_label = match self.editor_mode {
            palette::Mode::Dark => rox_i18n::t!("settings-appearance-inverse-from-light"),
            palette::Mode::Light => rox_i18n::t!("settings-appearance-inverse-from-dark"),
        };
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(small_button(
                inverse_label,
                icons::CONTRAST,
                locked,
                cx.listener(|this, _, window, cx| this.inverse_palette(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("panel-apply-song-theme"),
                icons::DISC,
                !locked,
                cx.listener(|this, _, window, cx| this.apply_song_theme(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("settings-appearance-palette-import"),
                icons::DOWNLOAD,
                locked,
                cx.listener(|this, _, window, cx| this.import_palette(window, cx)),
            ))
            .child(small_button(
                rox_i18n::t!("settings-appearance-palette-export"),
                icons::UPLOAD,
                false,
                cx.listener(|this, _, _, cx| this.export_palette(cx)),
            ))
            .child(small_button(
                rox_i18n::t!("panel-reset"),
                icons::REFRESH_CW,
                locked,
                cx.listener(|this, _, window, cx| this.reset_palette(window, cx)),
            ));
        Section::new(
            q,
            icons::PALETTE,
            rox_i18n::t!("settings-appearance-section-colors"),
            Some(controls.into_any_element()),
            |rows| {
                rows.custom(
                    &["palette", "accent", "swatch", "role", "import", "export"],
                    || {
                        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
                        if locked {
                            body = body.child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-appearance-colors-locked-note")),
                            );
                        }
                        body.child(settings_ui::role_grid(columns, |j| {
                            self.color_cell(&ROLES[j], &self.pickers[j], locked)
                                .into_any_element()
                        }))
                        .into_any_element()
                    },
                )
            },
        )
    }
}
