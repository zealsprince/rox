//! The Shader settings page: the whole-window post-process and the backdrop
//! shader, each with its source, the shared pool, the in-app editor and its
//! signal routes. Its own page because each is a program run every frame,
//! matching the panel settings window's Shader page.

use super::*;

impl SettingsWindow {
    /// A workspace apply swaps the screen shader wholesale, so re-seed the
    /// copies off the apply counter at render. This window's own edits move the
    /// counter too, landing back on what they wrote.
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
        // The apply already pushed the file's lists into the live feed, so the
        // editor has to show them too.
        self.post_shader_routes = config.routes;
        self.post_shader_manual = config.manual;
    }

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

        // The driver reads and watches the file itself, and an inline source
        // wins over the bookmark, so the bookmark only reads as the file choice
        // with nothing inlined. The picker only checks that something resolves,
        // so a stand-in spares a file read per render.
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
            // A scene over the whole window replaces the app, so only shaders
            // that declare they leave it usable are offered.
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
        // Warns before a scene hides the app, this row included; the confirm
        // window stays the real way out. Read off what the driver compiled.
        let covers = enabled && crate::workspace::post_shader_overlay() == Some(false);
        // Slot names come off the file the workspace compiled.
        let hub = self.signals.clone();
        let labels = crate::workspace::post_shader_slot_labels();
        // An unrouted slot is a hand-set knob, which is how a screen shader's
        // named parameters get tuned.
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
                        // A banner rather than a muted line: the switch above
                        // reads as on while nothing runs.
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
                            // Without this, the first route added would look like it
                            // broke the other fifteen slots.
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

    /// The file write waits for the burst to settle: it reserializes every
    /// shard, dock dumps and all.
    fn edit_post_shader_routes(
        &mut self,
        edit: &mut dyn FnMut(&mut Vec<Route>),
        cx: &mut Context<Self>,
    ) {
        edit(&mut self.post_shader_routes);
        let routes = self.post_shader_routes.clone();
        crate::workspace::set_post_shader_routes(routes.clone());
        // Its own generation, so a route drag never cancels a pending palette
        // write.
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

    /// Turning it on raises the Keep or Revert confirm: a shader can bury the
    /// toggle that would undo it.
    fn set_post_shader_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        let prior = Settings::load().post_shader;
        Settings::update(move |s| s.post_shader.enabled = on);
        crate::workspace::apply_post_shader(cx);
        // Anything that resolves to a source needs the confirm, whatever it
        // came from.
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

    /// `confirm` is false for moves that change where the text lives without
    /// changing what draws (detach, eject, save).
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

    fn clear_post_shader_source(&mut self, cx: &mut Context<Self>) {
        self.edit_post_shader_source(None, String::new(), None, false, cx);
    }

    fn use_post_shader_example(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(preset) = panel::shader::PRESETS.get(index) else {
            return;
        };
        self.edit_post_shader_source(None, preset.source.to_string(), None, true, cx);
    }

    /// Nothing is approved on the way through: a pool entry from a bundle still
    /// has to be read first.
    fn use_post_shader_pool(&mut self, name: String, cx: &mut Context<Self>) {
        self.edit_post_shader_source(Some(name), String::new(), None, true, cx);
    }

    /// A named shader edits the pool entry; anything else edits the inline
    /// text, seeded from the file in file mode. The write goes straight to
    /// settings, since the editor outlives this window. No confirm: an apply is
    /// the user's own text.
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
        let Some((_, state)) = rox_panel_api::windows::front_workspace(cx) else {
            return;
        };
        crate::shader_editor::open(state, target, cx);
    }

    /// The same text keeps running, so nothing to confirm.
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

    /// A named shader ejects through its pool entry. An inline one moves onto
    /// the written file, the one mode the screen driver's watch hot reloads.
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

    /// A file-mode config hands over the file's text, and the bookmark moves
    /// onto the pool entry.
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

    /// A pick turns nothing on by itself. The name and inline source are
    /// cleared, since both would win over the file.
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

    fn reload_post_shader(&mut self, cx: &mut Context<Self>) {
        crate::workspace::apply_post_shader(cx);
        cx.notify();
    }

    /// Absent reads as default with All Windows on; a look that leaves its
    /// children bare turns it off explicitly.
    fn backdrop_config() -> settings::PostShaderConfig {
        settings::backdrop_shader().unwrap_or_else(|| settings::PostShaderConfig {
            all_windows: true,
            ..Default::default()
        })
    }

    /// No confirm for the backdrop: the panels paint over it, so it can never
    /// bury its own switch. A config cleared back to nothing collapses to None,
    /// so exports carry no empty block.
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

    /// Written through the same cache, file and repaint as
    /// [`write_backdrop`](Self::write_backdrop), outside this window since the
    /// editor outlives it.
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
        let Some((_, state)) = rox_panel_api::windows::front_workspace(cx) else {
            return;
        };
        crate::shader_editor::open(state, target, cx);
    }

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

    /// An inline shader keeps its source and takes the file as a bookmark,
    /// which puts it under the surface's own watch.
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

    /// Inline rather than file mode: the panel surface machinery resolves a
    /// name or an inline source and nothing else.
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

    fn reload_backdrop(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = Self::backdrop_config().path {
            self.load_backdrop_file(path, cx);
        }
    }

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

    /// Painted between the art wash and the panels, so it stays under
    /// everything. Stored in the look's bundle and travels with the workspace.
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
            // Under the panels, nothing can take the app, so the list stays
            // unfiltered.
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
