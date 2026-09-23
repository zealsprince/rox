//! The Shader settings page: the whole-window post-process and the backdrop
//! shader, each with its source, the shared pool, the in-app editor, and its
//! signal routes.

use super::*;

impl SettingsWindow {
    /// Catch the Shader page's copies up to a config this window didn't
    /// write: a workspace apply swaps the screen shader wholesale, and the
    /// picker kept naming the one the old look used. Runs from render off
    /// the workspace's apply counter, `sync_editor_side`'s shape, since
    /// every apply repaints all windows. This window's own edits move the
    /// counter too, and end up back on the values they just wrote.
    pub(super) fn sync_post_shader(&mut self) {
        let generation = crate::workspace::post_shader_gen();
        if generation == self.post_shader_gen {
            return;
        }
        self.post_shader_gen = generation;
        let Some(config) = crate::workspace::post_shader_applied() else {
            return;
        };
        self.post_shader_name = config.name;
        self.post_shader_source = config.source;
        self.post_shader_path = config.path;
        self.post_shader_all_windows = config.all_windows;
        self.post_shader_run_idle = config.run_when_idle;
        // The routes and hand-set slots update too: the apply already
        // pushed the file's copies into the live feed the shader reads, so
        // leaving the editor on the old lists would show one thing and
        // drive another.
        self.post_shader_routes = config.routes;
        self.post_shader_manual = config.manual;
    }

    /// The Shader page: the whole-window post-process and what drives it.
    /// Its own page rather than a section under Appearance because it
    /// isn't a look setting: it's a program the app runs over every
    /// frame, with a file, a compile error, and sixteen signal routes,
    /// and it had already outgrown sitting between Transparency and
    /// Frame. Matches the panel settings window, where a panel's shader
    /// is its own page under the same icon.
    pub(super) fn shader_page(
        &mut self,
        q: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PageBody {
        PageBody::new()
            .section(self.screen_shader_section(q, window, cx))
            .section(self.backdrop_shader_section(q, window, cx))
    }

    /// The Screen Shader section: a WGSL post-process over the whole
    /// window, run by the workspace's driver. The toggle and source are
    /// written to settings and reapply everywhere; the error line reads
    /// the driver's live readout, so a broken edit caught by the hot
    /// reload shows here without a round trip through this window.
    fn screen_shader_section(
        &mut self,
        q: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Section {
        let enabled = crate::workspace::post_shader_on();
        let all_windows = self.post_shader_all_windows;
        let run_idle = self.post_shader_run_idle;
        let error = crate::workspace::post_shader_error();

        // The picker the panel shader pages lead with, over the app-wide
        // config, so the examples and the workspace's shaders are one list
        // wherever a shader gets picked. The file case diverges from the
        // panels in one way: the driver reads and watches the file itself,
        // and an inline source wins over the bookmark, so the bookmark only
        // reads as the file choice while nothing is inlined over it. The
        // picker only checks whether something resolves, so a stand-in
        // spares the section a file read per render.
        let name = self.post_shader_name.clone();
        let file_mode = name.is_none()
            && self.post_shader_source.trim().is_empty()
            && self.post_shader_path.is_some();
        let resolved = match name.as_deref() {
            Some(name) => settings::shader_pool_get(name).map(|entry| entry.source),
            None if !self.post_shader_source.trim().is_empty() => {
                Some(self.post_shader_source.clone())
            }
            None if file_mode => Some("// the file the driver watches".to_string()),
            None => None,
        };
        let path = file_mode.then(|| self.post_shader_path.clone()).flatten();
        let picked = ShaderSource {
            id: "screen-shader",
            name: name.as_deref(),
            path: path.as_deref(),
            resolved: resolved.as_deref(),
            clear: Some(|this: &mut Self, cx| this.clear_post_shader_source(cx)),
            // The whole window is the one surface where a scene doesn't
            // decorate the app, it replaces it, so the list only offers
            // shaders that declare they leave it usable.
            overlays_only: true,
            use_example: |this: &mut Self, index, cx| this.use_post_shader_example(index, cx),
            use_named: |this: &mut Self, name, cx| this.use_post_shader_pool(name, cx),
            choose_file: |this: &mut Self, window, cx| this.pick_post_shader(window, cx),
            edit: |this: &mut Self, window, cx| this.edit_post_shader_in_app(window, cx),
            eject: |this: &mut Self, cx| this.eject_post_shader(cx),
            detach: |this: &mut Self, cx| this.detach_post_shader(cx),
            reload: |this: &mut Self, cx| this.reload_post_shader(cx),
            save: |this: &mut Self, name, cx| this.save_post_shader_to_pool(name, cx),
            field: &mut self.post_shader_save_name,
            fallback: rox_i18n::t_static("settings-shader-screen-fallback-name"),
        }
        .render(window, cx);
        // A scene over the whole window hides the app, this row included.
        // The countdown is the way back out and stays the real safety net;
        // this line means nobody has to find that out by watching their
        // library disappear. Read off what's installed, so it applies to
        // the file the driver compiled as well as to a source this window
        // can see.
        let covers = enabled && crate::workspace::post_shader_overlay() == Some(false);
        // The same route editor the panel Shader page and the Shader
        // panel's Bindings page use, over the app-wide list. Its slot
        // names come off the file the workspace compiled, so a shader that
        // declares them reads the same here as it does on a panel.
        let hub = self.signals.clone();
        let labels = crate::workspace::post_shader_slot_labels();
        // The Bindings page's slot list over the app-wide config: a routed
        // slot shows the value going to the shader, an unrouted one is a
        // hand-set knob, which is how a screen shader's named parameters get
        // tuned without editing WGSL.
        let slots = signal_ui::slots::SlotList {
            hub: &hub,
            routes: &self.post_shader_routes,
            manual: &self.post_shader_manual,
            labels: &labels,
            value_edit: &self.value_edit,
            scrubs: &self.post_shader_slot_scrubs,
            set: Arc::new(|this: &mut Self, slot, value, cx| {
                this.set_post_shader_manual(slot, value, cx)
            }),
        }
        .render(cx);
        let editor = signal_ui::routes::RouteEditor {
            id: "screen-shader-route",
            hub: &hub,
            routes: &self.post_shader_routes,
            labels: &labels,
            value_edit: &self.value_edit,
            ui: &self.post_shader_route_ui,
            ui_mut: |this: &mut Self| &mut this.post_shader_route_ui,
            mutate: Arc::new(
                |this: &mut Self, edit: &mut dyn FnMut(&mut Vec<Route>), cx: &mut Context<Self>| {
                    this.edit_post_shader_routes(edit, cx);
                },
            ),
        };
        let legacy = self.post_shader_routes.is_empty();
        Section::new(
            q,
            icons::BLEND,
            rox_i18n::t!("settings-shader-section-overlay"),
            None,
            move |mut rows| {
                rows = rows.keyed(
                    "settings-shader-overlay-enabled",
                    &[
                        "shader",
                        "wgsl",
                        "post process",
                        "effect",
                        "crt",
                        "overlay",
                        "screen",
                    ],
                    panel::toggle(enabled, Self::set_post_shader_enabled, cx),
                );
                // The source, its scope and its error are all about a
                // shader that's running; off, the switch is the whole row.
                if !enabled {
                    return rows;
                }
                rows = rows
                    .custom(
                        &[
                            "shader",
                            "wgsl",
                            "file",
                            "reload",
                            "source",
                            "example",
                            "preset",
                            "workspace",
                        ],
                        || picked.into_any_element(),
                    )
                    .when(covers, |rows| {
                        rows.custom(&["shader", "scene", "covers", "hides", "overlay"], || {
                            coverage_note(
                                rox_i18n::t!("settings-shader-scene-covers-window").to_string(),
                            )
                            .into_any_element()
                        })
                    })
                    .keyed(
                        "settings-shader-screen-all-windows",
                        &["shader", "child windows", "settings", "everywhere"],
                        panel::toggle(all_windows, Self::set_post_shader_all_windows, cx),
                    )
                    .keyed(
                        "settings-shader-screen-run-idle",
                        &["shader", "idle", "pause", "freeze", "mouse"],
                        panel::toggle(run_idle, Self::set_post_shader_run_idle, cx),
                    );
                rows = match error {
                    Some(error) => rows.custom(&["shader", "error", "compile"], || {
                        // The callout the output section shows for a failed
                        // device, for the same reason: the switch above reads as
                        // on, and a muted line under it is not enough to say that
                        // nothing behind it is running. A backend with no shader
                        // pipeline rejects every source with one word, which
                        // on its own reads as a stray label rather than a reason.
                        match panel::shader::unsupported(&error) {
                            true => panel::banner(
                                panel::Tone::Bad,
                                panel::shader::NO_PIPELINE_TITLE,
                                vec![panel::shader::NO_PIPELINE_NOTE.into()],
                            ),
                            false => panel::banner(
                                panel::Tone::Bad,
                                rox_i18n::t!("settings-shader-compile-error-title"),
                                vec![error.into()],
                            ),
                        }
                        .into_any_element()
                    }),
                    None => rows,
                };
                rows.custom(
                    &[
                        "shader",
                        "signal",
                        "route",
                        "slot",
                        "bind",
                        "modulation",
                        "knob",
                        "manual",
                    ],
                    || {
                        let add = editor.add_button(cx);
                        let mut body = div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_MD)
                            .child(editor.list(cx));
                        if legacy {
                            // Nothing routed is not nothing happening here, and
                            // saying so is the only way the first route someone
                            // adds doesn't look like it broke the other fifteen
                            // slots.
                            body = body.child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("settings-shader-legacy-note")),
                            );
                        }
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_MD)
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-signals-block"),
                                Some(rox_i18n::t!("settings-shader-signals-block.description")),
                                Some(add.into_any_element()),
                                body,
                            ))
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-slots-block"),
                                Some(rox_i18n::t!("settings-shader-slots-block.description")),
                                None,
                                slots,
                            ))
                            .into_any_element()
                    },
                )
            },
        )
    }

    /// One edit to the screen shader's routes: into this window's copy,
    /// into the workspace's live feed so the shader follows the drag, and
    /// into the file once the burst settles. The file write waits because
    /// it reloads and reserializes every shard, dock dumps and all, which
    /// is not what a slider tick should cost.
    fn edit_post_shader_routes(
        &mut self,
        edit: &mut dyn FnMut(&mut Vec<Route>),
        cx: &mut Context<Self>,
    ) {
        edit(&mut self.post_shader_routes);
        let routes = self.post_shader_routes.clone();
        crate::workspace::set_post_shader_routes(routes.clone());
        // Its own generation, not the appearance one: a route drag must not
        // cancel a pending palette write, and the two bursts overlap the
        // moment someone tunes a shader against a color.
        self.route_persist_gen += 1;
        let generation = self.route_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.route_persist_gen)
                .unwrap_or(generation);
            if latest == generation {
                Settings::update(move |s| s.post_shader.routes = routes);
            }
        })
        .detach();
        cx.notify();
    }

    /// One hand-set slot edit: into this window's copy, into the
    /// workspace's live feed so the shader follows the drag, and into the
    /// file once the burst settles, the routes' exact write path.
    fn set_post_shader_manual(&mut self, slot: usize, value: f32, cx: &mut Context<Self>) {
        match self
            .post_shader_manual
            .iter_mut()
            .find(|(at, _)| *at as usize == slot)
        {
            Some(entry) => entry.1 = value,
            None => self.post_shader_manual.push((slot as u8, value)),
        }
        let manual = self.post_shader_manual.clone();
        crate::workspace::set_post_shader_manual(manual.clone());
        self.manual_persist_gen += 1;
        let generation = self.manual_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.manual_persist_gen)
                .unwrap_or(generation);
            if latest == generation {
                Settings::update(move |s| s.post_shader.manual = manual);
            }
        })
        .detach();
        cx.notify();
    }

    /// The shader switch: into the file, then every shaded window
    /// reapplies, which also clears the pass when it goes off.
    /// Turning it on runs the countdown confirm; a shader can bury the
    /// very toggle that would undo it, so the change has to prove itself
    /// or roll back on its own. Off needs no proof.
    fn set_post_shader_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let prior = Settings::load().post_shader;
        Settings::update(move |s| s.post_shader.enabled = on);
        crate::workspace::apply_post_shader(cx);
        // Anything that resolves to a source is worth proving, whether it
        // came from a file, from a bundle's inline copy, or from the
        // workspace's pool. Nothing to run needs no countdown.
        if on
            && crate::workspace::post_shader_source(&prior)
                .ok()
                .flatten()
                .is_some()
        {
            self.confirm_post_shader(prior, cx);
        }
        cx.notify();
    }

    /// The all-windows switch: no confirm of its own, the countdown window
    /// stays out of the shading regardless.
    fn set_post_shader_all_windows(&mut self, on: bool, cx: &mut Context<Self>) {
        self.post_shader_all_windows = on;
        Settings::update(move |s| s.post_shader.all_windows = on);
        crate::workspace::apply_post_shader(cx);
        cx.notify();
    }

    fn set_post_shader_run_idle(&mut self, on: bool, cx: &mut Context<Self>) {
        self.post_shader_run_idle = on;
        Settings::update(move |s| s.post_shader.run_when_idle = on);
        crate::workspace::apply_post_shader(cx);
        cx.notify();
    }

    /// One edit to the screen shader's source trio: the copies, the file,
    /// the reapply, and the countdown when the change applies to a running
    /// shader. Every picker action funnels through here; `confirm` is
    /// false for the moves that change where the text is stored without
    /// changing what draws (detach, eject, save), which have nothing for
    /// a countdown to revert.
    fn edit_post_shader_source(
        &mut self,
        name: Option<String>,
        source: String,
        path: Option<PathBuf>,
        confirm: bool,
        cx: &mut Context<Self>,
    ) {
        let prior = Settings::load().post_shader;
        self.post_shader_name = name.clone();
        self.post_shader_source = source.clone();
        self.post_shader_path = path.clone();
        Settings::update(move |s| {
            s.post_shader.name = name;
            s.post_shader.source = source;
            s.post_shader.path = path;
        });
        crate::workspace::apply_post_shader(cx);
        if confirm && prior.enabled {
            self.confirm_post_shader(prior, cx);
        }
        cx.notify();
    }

    /// Take the screen shader off whatever it was on: no name, no source,
    /// no bookmark. The switch above stays its own decision, the panel
    /// pages' split.
    fn clear_post_shader_source(&mut self, cx: &mut Context<Self>) {
        self.edit_post_shader_source(None, String::new(), None, false, cx);
    }

    /// Load one of the shipped examples. Builtin, so nothing to approve.
    fn use_post_shader_example(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(preset) = panel::shader::PRESETS.get(index) else {
            return;
        };
        self.edit_post_shader_source(None, preset.source.to_string(), None, true, cx);
    }

    /// Point the screen at one of the workspace's shaders. Nothing is
    /// approved on the way through, the same as a panel picking a name: a
    /// pool entry that arrived with a bundle still has to be read first.
    fn use_post_shader_pool(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_post_shader_source(Some(name), String::new(), None, true, cx);
    }

    /// Open the in-app editor over the screen shader. A named one edits
    /// the pool entry; anything else edits the inline text, seeded from
    /// the file in file mode, and an apply lands as an inline source with
    /// the file kept as its bookmark. The write goes straight to the
    /// settings and the reapply rather than through this window, since
    /// the editor outlives it; the generation counter brings this page's
    /// copies along. No countdown: an apply is the user's own text, the
    /// same trust a hot reload from their editor gets.
    fn edit_post_shader_in_app(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use panel::shader::edit::{EditKey, ShaderEditTarget};

        self.sync_post_shader();
        let target = match self.post_shader_name.as_deref() {
            Some(name) => ShaderEditTarget::pool(name),
            None => {
                let path = self.post_shader_path.clone();
                let source = if self.post_shader_source.trim().is_empty() {
                    path.as_deref()
                        .and_then(|path| std::fs::read_to_string(path).ok())
                        .unwrap_or_default()
                } else {
                    self.post_shader_source.clone()
                };
                let bookmark = path.clone();
                Some(ShaderEditTarget {
                    key: EditKey::Screen,
                    title: rox_i18n::t!("shader-editor-target-screen"),
                    source,
                    ctx: panel::shader::ProgramCtx::of(None, path.as_deref()),
                    path,
                    write: Arc::new(move |source, cx| {
                        let bookmark = bookmark.clone();
                        Settings::update(move |s| {
                            s.post_shader.name = None;
                            s.post_shader.source = source;
                            s.post_shader.path = bookmark;
                        });
                        crate::workspace::apply_post_shader(cx);
                    }),
                })
            }
        };
        let Some(target) = target else {
            return;
        };
        // The front workspace's state, the same bundle every other window
        // opened from here runs on.
        let Some((_, state)) = rox_panel_api::windows::front_workspace(cx) else {
            return;
        };
        crate::shader_editor::open(state, target, cx);
    }

    /// Take a private copy of the pool shader the screen is using. The
    /// same text keeps running, so there's nothing for a countdown to do.
    fn detach_post_shader(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self
            .post_shader_name
            .as_deref()
            .and_then(settings::shader_pool_get)
        else {
            return;
        };
        self.edit_post_shader_source(None, entry.source, None, false, cx);
    }

    /// Write the screen shader out to a file and hand it to whatever opens
    /// `.wgsl`, the panel pages' authoring loop. A named shader ejects
    /// through its pool entry, so the edits apply to every surface using
    /// the name; an inline one is written under the live workspace's
    /// shaders and the config moves onto the file, the one mode the
    /// screen driver's own watch hot reloads.
    fn eject_post_shader(&mut self, cx: &mut Context<Self>) {
        let config = Settings::load().post_shader;
        let ejected = match config.name.as_deref() {
            Some(name) => panel::shader::eject_pool_entry(name).map(|path| (path, false)),
            None if !config.source.trim().is_empty() => {
                let name = panel::shader::eject_name("Screen", &config.source);
                panel::shader::eject(&name, &config.source).map(|path| (path, true))
            }
            None => return,
        };
        match ejected {
            Ok((path, rebind)) => {
                if rebind {
                    self.edit_post_shader_source(
                        None,
                        String::new(),
                        Some(path.clone()),
                        false,
                        cx,
                    );
                }
                cx.open_with_system(&path);
            }
            Err(error) => {
                crate::workspace::note_post_shader_error(
                    rox_i18n::t!("shader-eject-failed", error = error.to_string()).to_string(),
                );
                cx.notify();
            }
        }
    }

    /// Promote the screen shader's own source into the workspace's shaders
    /// and use it by name from there, the panel pages' move. A file-mode
    /// config hands over the file's text and the bookmark moves onto the
    /// pool entry, so the authoring loop carries on through the pool's
    /// watch.
    fn save_post_shader_to_pool(&mut self, name: String, cx: &mut Context<Self>) {
        let config = Settings::load().post_shader;
        let name = name.trim().to_string();
        if name.is_empty() || config.name.is_some() {
            return;
        }
        let source = if !config.source.trim().is_empty() {
            config.source.clone()
        } else if let Some(path) = &config.path {
            match std::fs::read_to_string(path) {
                Ok(source) => source,
                Err(error) => {
                    crate::workspace::note_post_shader_error(format!(
                        "reading {}: {error}",
                        path.display()
                    ));
                    cx.notify();
                    return;
                }
            }
        } else {
            return;
        };
        panel::shader::save_to_pool(&name, &source, config.path.clone());
        self.edit_post_shader_source(Some(name), String::new(), None, false, cx);
    }

    /// Browse for the shader file. Picking one turns nothing on by itself;
    /// the toggle stays the one switch. A pick made while the shader
    /// runs takes visible effect, so that path runs the confirm too. The
    /// name and inline source go with it: both would win over the file at
    /// resolve time, so leaving either behind would make the pick a no-op.
    fn pick_post_shader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            this.update(cx, |this, cx| {
                this.edit_post_shader_source(None, String::new(), Some(path), true, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Put the just-applied shader on the countdown clock, with this
    /// window's copies refreshed if the clock wins.
    fn confirm_post_shader(&mut self, prior: settings::PostShaderConfig, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        crate::settings::shader_confirm::open(
            prior,
            self.player,
            self.now_art.clone(),
            move |cx| {
                weak.update(cx, |this, cx| {
                    let config = Settings::load().post_shader;
                    this.post_shader_name = config.name;
                    this.post_shader_source = config.source;
                    this.post_shader_path = config.path;
                    cx.notify();
                })
                .ok();
            },
            cx,
        );
    }

    /// Recompile the file as it stands, for shader edits the mtime watch
    /// missed (a same-second rewrite) or a nudge after fixing an error.
    fn reload_post_shader(&mut self, cx: &mut Context<Self>) {
        crate::workspace::apply_post_shader(cx);
        cx.notify();
    }

    /// The backdrop shader as the look holds it, the base of every read
    /// and edit on the Backdrop section. Absent reads as an untouched
    /// default with All Windows on: shading every backdrop is the whole-app
    /// read, and a look that leaves its children bare turns it off
    /// explicitly, the way Diffuse does.
    fn backdrop_config() -> settings::PostShaderConfig {
        settings::backdrop_shader().unwrap_or_else(|| settings::PostShaderConfig {
            all_windows: true,
            ..Default::default()
        })
    }

    /// One write to the backdrop config: the cache the workspace roots
    /// read, the look's bundle in the file, and a repaint so the shader
    /// follows the knob. No countdown confirm anywhere on this page: the
    /// panels paint over this pass whatever it does, so it can never bury
    /// the switch that would undo it.
    ///
    /// A config cleared all the way back to nothing collapses to None, so
    /// clearing the shader leaves no empty block in the look's exports.
    fn write_backdrop(&mut self, config: settings::PostShaderConfig, cx: &mut Context<Self>) {
        let config =
            (config.configured() || !config.routes.is_empty() || !config.manual.is_empty())
                .then_some(config);
        settings::note_backdrop_shader(config.clone());
        Settings::update(move |s| s.look.bundle.backdrop_shader = config);
        crate::workspace::refresh_backdrop(cx);
        cx.notify();
    }

    fn set_backdrop_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        config.enabled = on;
        self.write_backdrop(config, cx);
    }

    fn set_backdrop_run_idle(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        config.run_when_idle = on;
        self.write_backdrop(config, cx);
    }

    fn set_backdrop_all_windows(&mut self, on: bool, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        config.all_windows = on;
        self.write_backdrop(config, cx);
    }

    /// One edit to the backdrop's source trio. Every picker action funnels
    /// through here, the screen shader's shape without the countdown.
    fn edit_backdrop_source(
        &mut self,
        name: Option<String>,
        source: String,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let mut config = Self::backdrop_config();
        config.name = name;
        config.source = source;
        config.path = path;
        self.write_backdrop(config, cx);
    }

    fn clear_backdrop_source(&mut self, cx: &mut Context<Self>) {
        self.edit_backdrop_source(None, String::new(), None, cx);
    }

    fn use_backdrop_example(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(preset) = panel::shader::PRESETS.get(index) else {
            return;
        };
        self.edit_backdrop_source(None, preset.source.to_string(), None, cx);
    }

    fn use_backdrop_pool(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_backdrop_source(Some(name), String::new(), None, cx);
    }

    /// Open the in-app editor over the backdrop shader, the screen
    /// shader's twin: a name edits the pool entry, anything else the
    /// inline text, written through the same cache-file-repaint trio as
    /// [`write_backdrop`](Self::write_backdrop), outside this window so
    /// the editor outlives it.
    fn edit_backdrop_in_app(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        use panel::shader::edit::{EditKey, ShaderEditTarget};

        let config = Self::backdrop_config();
        let target = match config.name.as_deref() {
            Some(name) => ShaderEditTarget::pool(name),
            None => {
                let path = config.path.clone();
                let source = if config.source.trim().is_empty() {
                    path.as_deref()
                        .and_then(|path| std::fs::read_to_string(path).ok())
                        .unwrap_or_default()
                } else {
                    config.source.clone()
                };
                let bookmark = path.clone();
                Some(ShaderEditTarget {
                    key: EditKey::Backdrop,
                    title: rox_i18n::t!("shader-editor-target-backdrop"),
                    source,
                    ctx: panel::shader::ProgramCtx::of(None, path.as_deref()),
                    path,
                    write: Arc::new(move |source, cx| {
                        let mut config = Self::backdrop_config();
                        config.name = None;
                        config.source = source;
                        config.path = bookmark.clone();
                        let config = Some(config);
                        settings::note_backdrop_shader(config.clone());
                        Settings::update(move |s| s.look.bundle.backdrop_shader = config);
                        crate::workspace::refresh_backdrop(cx);
                    }),
                })
            }
        };
        let Some(target) = target else {
            return;
        };
        // The front workspace's state, the same bundle every other window
        // opened from here runs on.
        let Some((_, state)) = rox_panel_api::windows::front_workspace(cx) else {
            return;
        };
        crate::shader_editor::open(state, target, cx);
    }

    /// Take a private copy of the pool shader the backdrop is using.
    fn detach_backdrop(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = Self::backdrop_config()
            .name
            .as_deref()
            .and_then(settings::shader_pool_get)
        else {
            return;
        };
        self.edit_backdrop_source(None, entry.source, None, cx);
    }

    /// Write the backdrop shader out to a file and hand it to whatever
    /// opens `.wgsl`. A named shader ejects through its pool entry; an
    /// inline one keeps its source and takes the file as a bookmark, which
    /// puts it under the surface's own watch, the panel pages' loop.
    fn eject_backdrop(&mut self, cx: &mut Context<Self>) {
        let config = Self::backdrop_config();
        match config.name.as_deref() {
            Some(name) => match panel::shader::eject_pool_entry(name) {
                Ok(path) => cx.open_with_system(&path),
                Err(error) => {
                    crate::workspace::note_backdrop_shader_error(
                        rox_i18n::t!("shader-eject-failed", error = error.to_string()).to_string(),
                    );
                    cx.notify();
                }
            },
            None if !config.source.trim().is_empty() => {
                let name = panel::shader::eject_name("Backdrop", &config.source);
                match panel::shader::eject(&name, &config.source) {
                    Ok(path) => {
                        self.edit_backdrop_source(
                            None,
                            config.source.clone(),
                            Some(path.clone()),
                            cx,
                        );
                        cx.open_with_system(&path);
                    }
                    Err(error) => {
                        crate::workspace::note_backdrop_shader_error(
                            rox_i18n::t!("shader-eject-failed", error = error.to_string())
                                .to_string(),
                        );
                        cx.notify();
                    }
                }
            }
            None => {}
        }
    }

    /// Promote the backdrop's own source into the workspace's shaders and
    /// use it by name from there.
    fn save_backdrop_to_pool(&mut self, name: String, cx: &mut Context<Self>) {
        let config = Self::backdrop_config();
        let name = name.trim().to_string();
        if name.is_empty() || config.name.is_some() || config.source.trim().is_empty() {
            return;
        }
        panel::shader::save_to_pool(&name, &config.source, config.path.clone());
        self.edit_backdrop_source(Some(name), String::new(), None, cx);
    }

    fn pick_backdrop_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            this.update(cx, |this, cx| this.load_backdrop_file(path, cx))
                .ok();
        })
        .detach();
    }

    /// Read a file into the config's inline source, with the path as the
    /// bookmark the surface watches. Inline rather than file-mode: the
    /// backdrop runs through the panel surface machinery, which resolves
    /// a name or an inline source and nothing else.
    fn load_backdrop_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                panel::shader::approve(&source);
                self.edit_backdrop_source(None, source, Some(path), cx);
            }
            Err(error) => {
                crate::workspace::note_backdrop_shader_error(format!(
                    "reading {}: {error}",
                    path.display()
                ));
                cx.notify();
            }
        }
    }

    /// Re-read the file behind the shader, for an edit the watch missed.
    fn reload_backdrop(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = Self::backdrop_config().path {
            self.load_backdrop_file(path, cx);
        }
    }

    /// One edit to the backdrop's routes: into the cache so the shader
    /// follows the drag, into the file once the burst settles.
    fn edit_backdrop_routes(
        &mut self,
        edit: &mut dyn FnMut(&mut Vec<Route>),
        cx: &mut Context<Self>,
    ) {
        let mut config = Self::backdrop_config();
        edit(&mut config.routes);
        settings::note_backdrop_shader(Some(config.clone()));
        crate::workspace::refresh_backdrop(cx);
        self.backdrop_route_persist_gen += 1;
        let generation = self.backdrop_route_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.backdrop_route_persist_gen)
                .unwrap_or(generation);
            if latest == generation {
                Settings::update(move |s| s.look.bundle.backdrop_shader = Some(config));
            }
        })
        .detach();
        cx.notify();
    }

    /// One hand-set slot edit, the routes' exact write path.
    fn set_backdrop_manual(&mut self, slot: usize, value: f32, cx: &mut Context<Self>) {
        let mut config = Self::backdrop_config();
        panel::shader::set_manual_value(&mut config.manual, slot, value);
        settings::note_backdrop_shader(Some(config.clone()));
        crate::workspace::refresh_backdrop(cx);
        self.backdrop_manual_persist_gen += 1;
        let generation = self.backdrop_manual_persist_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let latest = this
                .update(cx, |this, _| this.backdrop_manual_persist_gen)
                .unwrap_or(generation);
            if latest == generation {
                Settings::update(move |s| s.look.bundle.backdrop_shader = Some(config));
            }
        })
        .detach();
        cx.notify();
    }

    /// The Backdrop Shader section: the same surface machinery a panel
    /// uses, painted between the art wash and the panels, so whatever it
    /// does stays under the whole window. It's stored in the look's bundle
    /// rather than the machine settings and travels with the workspace.
    fn backdrop_shader_section(
        &mut self,
        q: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Section {
        let config = Self::backdrop_config();
        let enabled = config.enabled;
        let all_windows = config.all_windows;
        let run_idle = config.run_when_idle;
        let error = crate::workspace::backdrop_shader_error();
        let resolved = match config.name.as_deref() {
            Some(name) => settings::shader_pool_get(name).map(|entry| entry.source),
            None => (!config.source.trim().is_empty()).then(|| config.source.clone()),
        };
        let labels = panel::shader::slot_labels(resolved.as_deref().unwrap_or_default());
        let path = config.name.is_none().then(|| config.path.clone()).flatten();
        let picked = ShaderSource {
            id: "backdrop-shader",
            name: config.name.as_deref(),
            path: path.as_deref(),
            resolved: resolved.as_deref(),
            clear: Some(|this: &mut Self, cx| this.clear_backdrop_source(cx)),
            // Everything here paints under the panels, so nothing it does
            // can take the app: the list stays unfiltered, scenes and all.
            overlays_only: false,
            use_example: |this: &mut Self, index, cx| this.use_backdrop_example(index, cx),
            use_named: |this: &mut Self, name, cx| this.use_backdrop_pool(name, cx),
            choose_file: |this: &mut Self, window, cx| this.pick_backdrop_file(window, cx),
            edit: |this: &mut Self, window, cx| this.edit_backdrop_in_app(window, cx),
            eject: |this: &mut Self, cx| this.eject_backdrop(cx),
            detach: |this: &mut Self, cx| this.detach_backdrop(cx),
            reload: |this: &mut Self, cx| this.reload_backdrop(cx),
            save: |this: &mut Self, name, cx| this.save_backdrop_to_pool(name, cx),
            field: &mut self.backdrop_save_name,
            fallback: rox_i18n::t_static("settings-shader-backdrop-fallback-name"),
        }
        .render(window, cx);
        let hub = self.signals.clone();
        let slots = signal_ui::slots::SlotList {
            hub: &hub,
            routes: &config.routes,
            manual: &config.manual,
            labels: &labels,
            value_edit: &self.value_edit,
            scrubs: &self.backdrop_slot_scrubs,
            set: Arc::new(|this: &mut Self, slot, value, cx| {
                this.set_backdrop_manual(slot, value, cx)
            }),
        }
        .render(cx);
        let editor = signal_ui::routes::RouteEditor {
            id: "backdrop-shader-route",
            hub: &hub,
            routes: &config.routes,
            labels: &labels,
            value_edit: &self.value_edit,
            ui: &self.backdrop_route_ui,
            ui_mut: |this: &mut Self| &mut this.backdrop_route_ui,
            mutate: Arc::new(
                |this: &mut Self, edit: &mut dyn FnMut(&mut Vec<Route>), cx: &mut Context<Self>| {
                    this.edit_backdrop_routes(edit, cx);
                },
            ),
        };
        Section::new(
            q,
            icons::LAYERS,
            rox_i18n::t!("settings-shader-section-backdrop"),
            None,
            move |mut rows| {
                rows = rows.keyed(
                    "settings-shader-backdrop-enabled",
                    &["shader", "wgsl", "backdrop", "wash", "art", "bokeh"],
                    panel::toggle(enabled, Self::set_backdrop_enabled, cx),
                );
                // The source, its scope, its error and its routes are all
                // about a shader that's running; off, the switch is the
                // whole row.
                if !enabled {
                    return rows;
                }
                rows = rows
                    .custom(
                        &[
                            "shader",
                            "wgsl",
                            "file",
                            "reload",
                            "source",
                            "example",
                            "preset",
                            "workspace",
                        ],
                        || picked.into_any_element(),
                    )
                    .keyed(
                        "settings-shader-backdrop-all-windows",
                        &[
                            "shader",
                            "child windows",
                            "settings",
                            "everywhere",
                            "backdrop",
                        ],
                        panel::toggle(all_windows, Self::set_backdrop_all_windows, cx),
                    )
                    .keyed(
                        "settings-shader-backdrop-run-idle",
                        &["shader", "idle", "pause", "freeze"],
                        panel::toggle(run_idle, Self::set_backdrop_run_idle, cx),
                    );
                rows = match error {
                    Some(error) => rows.custom(&["shader", "error", "compile"], || {
                        match panel::shader::unsupported(&error) {
                            true => panel::banner(
                                panel::Tone::Bad,
                                panel::shader::NO_PIPELINE_TITLE,
                                vec![panel::shader::NO_PIPELINE_NOTE.into()],
                            ),
                            false => panel::banner(
                                panel::Tone::Bad,
                                rox_i18n::t!("settings-shader-compile-error-title"),
                                vec![error.into()],
                            ),
                        }
                        .into_any_element()
                    }),
                    None => rows,
                };
                rows.custom(
                    &[
                        "shader",
                        "signal",
                        "route",
                        "slot",
                        "bind",
                        "modulation",
                        "knob",
                        "manual",
                    ],
                    || {
                        let add = editor.add_button(cx);
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_MD)
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-signals-block"),
                                Some(rox_i18n::t!("settings-shader-signals-block.description")),
                                Some(add.into_any_element()),
                                editor.list(cx),
                            ))
                            .child(panel::setting_block(
                                rox_i18n::t!("settings-shader-slots-block"),
                                Some(rox_i18n::t!("settings-shader-slots-block.description")),
                                None,
                                slots,
                            ))
                            .into_any_element()
                    },
                )
            },
        )
    }
}
