//! Subsonic servers on the Library page: a row each in the sources table, and a
//! setup dialog behind it.
//!
//! A server's position in [`SettingsWindow::subsonic`] is its account's index
//! in accounts.json, and the two only move together. A callback finds its index
//! again by id first, since one above it may have gone meanwhile.

use super::*;

use gpui::Focusable;
use rox_core::settings::SubsonicAccount;

/// Slower than `RG_POLL`: the count moves once per album, a request to somebody
/// else's server.
const SUBSONIC_SYNC_POLL: Duration = Duration::from_millis(500);

/// The dialog isn't searchable, so the sources table carries the server
/// settings' terms.
pub(super) const KEYWORDS: &[&str] = &[
    "subsonic",
    "opensubsonic",
    "navidrome",
    "airsonic",
    "gonic",
    "server",
    "url",
    "host",
    "address",
    "user",
    "password",
    "login",
    "credentials",
    "connect",
    "test",
    "ping",
    "sync",
    "catalog",
    "refresh",
    "delete",
    "forget",
];

type Write = fn(&mut SubsonicAccount, String);

pub(crate) struct SubsonicForm {
    /// Stable while the window is open, unlike the position. Element ids, the
    /// dialog and the remove confirm key on it.
    id: u64,
    name: Entity<InputState>,
    url: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    enabled: bool,
    /// Added from the menu and not confirmed yet: out of the table, and
    /// cancelling removes it from the file.
    fresh: bool,
    status: Option<SharedString>,
    /// Read at open and after anything that moves rows, never per frame.
    stats: Stats,
    last_sync: i64,
    /// A field moved since the library last followed the accounts. The name
    /// counts: the catalog reads server names on its next load.
    dirty: bool,
    _changes: Vec<Subscription>,
}

impl SubsonicForm {
    pub(crate) fn new(
        id: u64,
        account: &SubsonicAccount,
        library: &Entity<Library>,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> Self {
        let mut field = |placeholder: &'static str, value: &str, masked: bool| {
            let value = value.to_string();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(rox_i18n::t!(placeholder))
                    .masked(masked)
                    .default_value(value)
            })
        };

        let name = field(
            "settings-integrations-subsonic-name-placeholder",
            &account.name,
            false,
        );
        let url = field(
            "settings-integrations-subsonic-url-placeholder",
            &account.url,
            false,
        );
        let user = field(
            "settings-integrations-subsonic-user-placeholder",
            &account.user,
            false,
        );
        let password = field(
            "settings-integrations-subsonic-password-placeholder",
            &account.password,
            true,
        );

        let fields: [(&Entity<InputState>, Write, bool); 4] = [
            (&name, |a, value| a.name = value.trim().to_string(), false),
            (&url, |a, value| a.url = value.trim().to_string(), true),
            (&user, |a, value| a.user = value.trim().to_string(), true),
            // Stored exactly as typed: a password can end in a space.
            (&password, |a, value| a.password = value, true),
        ];

        let _changes = fields
            .into_iter()
            .map(|(input, write, moves)| {
                let on_event =
                    move |this: &mut SettingsWindow,
                          input: Entity<InputState>,
                          event: &InputEvent,
                          cx: &mut Context<SettingsWindow>| {
                        this.subsonic_field_event(id, &input, event, write, moves, cx)
                    };

                cx.subscribe(input, on_event)
            })
            .collect();

        SubsonicForm {
            id,
            name,
            url,
            user,
            password,
            enabled: account.enabled,
            fresh: false,
            status: None,
            stats: source_stats(library, account, cx),
            last_sync: account.last_sync,
            dirty: false,
            _changes,
        }
    }

    fn account(&self, cx: &App) -> SubsonicAccount {
        SubsonicAccount {
            enabled: self.enabled,
            name: self.name.read(cx).value().to_string(),
            url: self.url.read(cx).value().trim().to_string(),
            user: self.user.read(cx).value().trim().to_string(),
            password: self.password.read(cx).value().to_string(),
            last_sync: self.last_sync,
        }
    }

    fn label(&self, cx: &App) -> SharedString {
        let label = self.account(cx).label();
        if label.is_empty() {
            return rox_i18n::t!("settings-integrations-subsonic-new");
        }

        SharedString::from(label)
    }

    fn addressed(&self, cx: &App) -> bool {
        !self.url.read(cx).value().trim().is_empty()
    }
}

impl SettingsWindow {
    /// A row reads like a folder's, with the server's standing beside its name.
    pub(super) fn subsonic_rows(&self, cx: &mut Context<Self>) -> Vec<Stateful<Div>> {
        (0..self.subsonic.len())
            .filter(|&ix| !self.subsonic[ix].fresh)
            .map(|ix| self.subsonic_row(ix, cx))
            .collect()
    }

    fn subsonic_row(&self, ix: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let form = &self.subsonic[ix];
        let id = form.id;
        let stats = form.stats;

        let name = div()
            .id(SharedString::from(format!("subsonic-name-{id}")))
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.open_subsonic(id, cx)))
            .child(
                svg()
                    .path(icons::DATABASE)
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::text_muted()),
            )
            .child(div().min_w_0().truncate().child(form.label(cx)))
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(self.subsonic_standing(ix, cx)),
            );

        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .child(icon_button(
                icons::PENCIL,
                false,
                cx.listener(move |this, _, _, cx| this.open_subsonic(id, cx)),
            ))
            .child(icon_button(
                icons::CLOSE,
                self.subsonic_syncing,
                cx.listener(move |this, _, _, cx| {
                    this.pending = Some(Pending::RemoveSubsonic(id));
                    cx.notify();
                }),
            ));

        div()
            .id(SharedString::from(format!("subsonic-row-{id}")))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .when(!form.enabled, |row| row.text_color(palette::text_muted()))
            .child(name)
            .child(number_cell(TRACKS_COL_W, stats.tracks.to_string()))
            .child(number_cell(ALBUMS_COL_W, stats.albums.to_string()))
            .child(number_cell(SIZE_COL_W, human_size(stats.bytes)))
            .child(action_cell(actions))
    }

    fn subsonic_standing(&self, ix: usize, cx: &App) -> SharedString {
        let form = &self.subsonic[ix];

        if !form.enabled {
            return rox_i18n::t!("settings-library-source-off");
        }

        if let Some((done, total)) = self.subsonic_progress(ix, cx) {
            return rox_i18n::t!(
                "settings-integrations-subsonic-syncing",
                done = done as i64,
                total = total as i64
            );
        }

        if form.last_sync == 0 {
            return rox_i18n::t!("settings-integrations-subsonic-sync-never");
        }

        rox_i18n::t!(
            "settings-library-source-synced",
            date = rox_core::fmt::fmt_date(form.last_sync)
        )
    }

    fn subsonic_progress(&self, ix: usize, cx: &App) -> Option<(usize, usize)> {
        let source = rox_services::sources::syncing_source()?;
        let mine = rox_services::sources::source_of(&self.subsonic[ix].account(cx))?;

        (source == mine)
            .then(rox_services::sources::progress)
            .flatten()
    }

    /// A fresh server's dialog asks Add or Cancel; an existing one just closes,
    /// since every field has written through. Escape answers like the quiet
    /// button.
    pub(super) fn subsonic_dialog(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let id = self.subsonic_editing?;
        let ix = self.subsonic_index(id)?;
        let form = &self.subsonic[ix];

        let title = match form.fresh {
            true => rox_i18n::t!("settings-library-subsonic-add-title"),
            false => form.label(cx),
        };

        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(!form.fresh, |d| {
                d.child(dialog_button(
                    rox_i18n::t!("settings-integrations-subsonic-remove"),
                    false,
                    cx.listener(move |this, _, _, cx| {
                        this.pending = Some(Pending::RemoveSubsonic(id));
                        cx.notify();
                    }),
                ))
            })
            .child(div().flex_1())
            .when(form.fresh, |d| {
                let addressed = form.addressed(cx);

                d.child(dialog_button(
                    rox_i18n::t!("settings-common-cancel"),
                    false,
                    cx.listener(move |this, _, _, cx| this.close_subsonic(cx)),
                ))
                .child(
                    div()
                        .when(!addressed, |d| d.opacity(0.5))
                        .child(dialog_button(
                            rox_i18n::t!("settings-common-add"),
                            true,
                            cx.listener(move |this, _, _, cx| this.confirm_subsonic(id, cx)),
                        )),
                )
            })
            .when(!form.fresh, |d| {
                d.child(dialog_button(
                    rox_i18n::t!("settings-common-done"),
                    true,
                    cx.listener(move |this, _, _, cx| this.close_subsonic(cx)),
                ))
            });

        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_subsonic(cx);
                        cx.stop_propagation();
                    }
                }))
                .bg(gpui::rgba(0x00000066))
                .child(
                    div()
                        .id(SharedString::from(format!("subsonic-dialog-{id}")))
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .w(px(560.))
                        .p(tokens::SPACE_MD)
                        .rounded(tokens::RADIUS)
                        // The page's floor rather than the menu fill, where a
                        // switch's track would vanish.
                        .bg(palette::bg_root_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(tokens::SPACE_XS)
                                .child(title)
                                // "Subsonic" names the API, so the add dialog says which
                                // servers it means.
                                .when(form.fresh, |d| {
                                    d.child(
                                        div().text_xs().text_color(palette::text_muted()).child(
                                            rox_i18n::t!("settings-library-subsonic-add-note"),
                                        ),
                                    )
                                }),
                        )
                        .child(self.subsonic_fields(ix, cx))
                        .child(buttons),
                ),
        )
    }

    /// A fresh server has no switch: it's being added to be used.
    fn subsonic_fields(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        let form = &self.subsonic[ix];
        let id = form.id;

        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .when(!form.fresh, |block| {
                block.child(block_row(
                    "settings-integrations-subsonic-use",
                    panel::toggle(
                        form.enabled,
                        move |this: &mut Self, on, cx| this.set_subsonic_enabled(id, on, cx),
                        cx,
                    ),
                ))
            })
            .when(form.enabled, |block| {
                block
                    .child(block_row(
                        "settings-integrations-subsonic-name",
                        Input::new(&form.name).w(px(260.)),
                    ))
                    .child(block_row(
                        "settings-integrations-subsonic-server",
                        Input::new(&form.url).w(px(260.)),
                    ))
                    .child(block_row(
                        "settings-integrations-subsonic-credentials",
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(Input::new(&form.user).w(px(120.)))
                            .child(Input::new(&form.password).mask_toggle().w(px(140.))),
                    ))
                    .child(self.subsonic_connect_row(ix, cx))
                    // Adding starts the first sync itself.
                    .when(!form.fresh, |block| {
                        block.child(self.subsonic_sync_row(ix, cx))
                    })
            })
    }

    /// Inert until there's a URL to reach.
    fn subsonic_connect_row(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        let form = &self.subsonic[ix];
        let id = form.id;

        let status = form
            .status
            .clone()
            .unwrap_or_else(|| rox_i18n::t!("settings-integrations-scrobble-status-not-connected"));

        panel::setting_row(
            status,
            None,
            small_button(
                rox_i18n::t!("settings-integrations-subsonic-connect"),
                icons::LINK,
                !form.addressed(cx),
                cx.listener(move |this, _, _, cx| this.subsonic_connect(id, cx)),
            ),
        )
    }

    fn subsonic_sync_row(&self, ix: usize, cx: &mut Context<Self>) -> Div {
        let form = &self.subsonic[ix];
        let id = form.id;

        let state: SharedString = match self.subsonic_progress(ix, cx) {
            Some((done, total)) => rox_i18n::t!(
                "settings-integrations-subsonic-syncing",
                done = done as i64,
                total = total as i64
            ),

            None if form.last_sync == 0 => {
                rox_i18n::t!("settings-integrations-subsonic-sync-never")
            }

            None => rox_i18n::t!(
                "settings-integrations-subsonic-sync-count",
                n = form.stats.tracks as i64,
                date = rox_core::fmt::fmt_date(form.last_sync)
            ),
        };

        panel::setting_row(
            state,
            Some(rox_i18n::t!(
                "settings-integrations-subsonic-sync-now.description"
            )),
            small_button(
                rox_i18n::t!("settings-integrations-subsonic-sync-now"),
                icons::REFRESH_CW,
                self.subsonic_syncing || !form.addressed(cx),
                cx.listener(move |this, _, _, cx| this.subsonic_sync(id, cx)),
            ),
        )
    }

    /// New Server for one that's gone, which only a confirm left open across a
    /// removal could ask about.
    pub(super) fn subsonic_label(&self, id: u64, cx: &App) -> SharedString {
        match self.subsonic_index(id) {
            Some(ix) => self.subsonic[ix].label(cx),
            None => rox_i18n::t!("settings-integrations-subsonic-new"),
        }
    }

    fn subsonic_index(&self, id: u64) -> Option<usize> {
        self.subsonic.iter().position(|form| form.id == id)
    }

    /// Leaving the field is the commit; see `subsonic_moved` for why not the
    /// keystroke.
    fn subsonic_field_event(
        &mut self,
        id: u64,
        input: &Entity<InputState>,
        event: &InputEvent,
        write: Write,
        moves: bool,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let value = input.read(cx).value().to_string();
                self.subsonic_write(id, value, write, moves, cx);
            }

            InputEvent::Blur | InputEvent::PressEnter { .. } => self.subsonic_moved(cx),

            InputEvent::Focus => {}
        }
    }

    /// A field that moves where the server points clears the last Connect's
    /// status.
    fn subsonic_write(
        &mut self,
        id: u64,
        value: String,
        write: Write,
        moves: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.subsonic_index(id) else {
            return;
        };

        Settings::update(move |s| {
            if let Some(account) = s.accounts.subsonic_servers.get_mut(ix) {
                write(account, value);
            }
        });

        let form = &mut self.subsonic[ix];
        form.dirty = true;
        if moves {
            form.status = None;
        }

        cx.notify();
    }

    /// Switched on so its fields show. The server stays out of the table until
    /// the dialog's Add confirms it.
    pub(super) fn add_subsonic(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let account = SubsonicAccount {
            enabled: true,
            ..SubsonicAccount::default()
        };

        let stored = account.clone();
        Settings::update(move |s| s.accounts.subsonic_servers.push(stored));

        let id = self.subsonic_next_id;
        self.subsonic_next_id += 1;

        let mut form = SubsonicForm::new(id, &account, &self.library, window, cx);
        form.fresh = true;
        window.focus(&form.url.read(cx).focus_handle(cx));

        self.subsonic.push(form);
        self.subsonic_editing = Some(id);

        cx.notify();
    }

    fn open_subsonic(&mut self, id: u64, cx: &mut Context<Self>) {
        self.subsonic_editing = Some(id);
        cx.notify();
    }

    /// A fresh server goes back out of the file. An existing one gets the
    /// commit a blur would have run.
    fn close_subsonic(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.subsonic_editing.take() else {
            return;
        };

        match self.subsonic_index(id) {
            // A running sync holds removals back, so the server joins the table
            // instead of sitting in the file with nothing on screen owning it.
            Some(ix) if self.subsonic[ix].fresh && rox_services::sources::syncing() => {
                self.subsonic[ix].fresh = false;
            }

            Some(ix) if self.subsonic[ix].fresh => self.remove_subsonic(id, cx),

            Some(_) => self.subsonic_moved(cx),

            None => {}
        }

        cx.notify();
    }

    /// Straight into its first sync.
    fn confirm_subsonic(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.subsonic_index(id) else {
            return;
        };

        if !self.subsonic[ix].addressed(cx) {
            return;
        }

        self.subsonic[ix].fresh = false;
        self.subsonic_editing = None;
        self.subsonic_moved(cx);
        self.subsonic_sync(id, cx);

        cx.notify();
    }

    /// Refused while a sync runs, like the service: dropping state without the
    /// account would point every later server at the wrong account.
    pub(super) fn remove_subsonic(&mut self, id: u64, cx: &mut Context<Self>) {
        if rox_services::sources::syncing() {
            return;
        }

        let Some(ix) = self.subsonic_index(id) else {
            return;
        };

        let pass = rox_services::sources::remove(ix, self.library.clone(), cx);
        self.subsonic.remove(ix);

        if self.subsonic_editing == Some(id) {
            self.subsonic_editing = None;
        }

        let this = cx.weak_entity();
        self.subsonic_follow = Some(follow(this, pass, cx));

        cx.notify();
    }

    /// The auth table follows the switch without a restart; off hides the
    /// server's tracks without deleting them.
    fn set_subsonic_enabled(&mut self, id: u64, on: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.subsonic_index(id) else {
            return;
        };

        self.subsonic[ix].enabled = on;
        Settings::update(move |s| {
            if let Some(account) = s.accounts.subsonic_servers.get_mut(ix) {
                account.enabled = on;
            }
        });

        rox_services::sources::install_registry();

        let pass = rox_services::sources::follow_accounts(self.library.clone(), cx);
        let this = cx.weak_entity();
        self.subsonic_follow = Some(follow(this, pass, cx));

        cx.notify();
    }

    /// Rows filed under an address an account has left can't be signed, so they
    /// go. On blur, not per keystroke: mid-edit every character is its own
    /// source id, which would drop the catalog on the first one. Nothing here
    /// dials a server.
    fn subsonic_moved(&mut self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        if let Some(task) = self.subsonic_commit(this, cx) {
            self.subsonic_follow = Some(task);
        }
    }

    /// `None` when nothing moved. The window handle comes in weak because the
    /// flush runs this while the entity is on its way out, and the prune must
    /// still happen.
    pub(super) fn subsonic_commit(
        &mut self,
        this: WeakEntity<Self>,
        cx: &mut App,
    ) -> Option<Task<()>> {
        if !self.subsonic.iter().any(|form| form.dirty) {
            return None;
        }

        for form in &mut self.subsonic {
            form.dirty = false;
        }

        // The table first, so a stream can be signed under a new source id
        // without a restart.
        rox_services::sources::install_registry();

        let pass = rox_services::sources::follow_accounts(self.library.clone(), cx);

        Some(follow(this, pass, cx))
    }

    fn refresh_subsonic_rows(&mut self, cx: &App) {
        let accounts = Settings::load().accounts.subsonic_servers;

        for (form, account) in self.subsonic.iter_mut().zip(&accounts) {
            form.stats = source_stats(&self.library, account, cx);
            form.last_sync = account.last_sync;
        }
    }

    fn subsonic_connect(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.subsonic_index(id) else {
            return;
        };

        let ping = rox_services::sources::ping(ix, cx);

        cx.spawn(async move |this, cx| {
            let answer = ping.await;

            this.update(cx, |this, cx| {
                let Some(ix) = this.subsonic_index(id) else {
                    return;
                };

                this.subsonic[ix].status = Some(match answer {
                    // A plain Subsonic server reports no name, only a protocol
                    // version.
                    Ok(info) if info.server_type.is_empty() => rox_i18n::t!(
                        "settings-integrations-subsonic-status-ok",
                        server = info.version
                    ),

                    Ok(info) => rox_i18n::t!(
                        "settings-integrations-subsonic-status-ok",
                        server = format!("{} {}", info.server_type, info.server_version)
                    ),

                    Err(e) => {
                        rox_i18n::t!("settings-integrations-subsonic-status-failed", error = e)
                    }
                });
                cx.notify();
            })
            .ok();
        })
        .detach();

        cx.notify();
    }

    /// A second task keeps the album count moving while it walks.
    fn subsonic_sync(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.subsonic_syncing {
            return;
        }

        let Some(ix) = self.subsonic_index(id) else {
            return;
        };

        self.subsonic_syncing = true;
        self.subsonic[ix].status = None;

        let sync = rox_services::sources::sync(ix, self.library.clone(), cx);

        cx.spawn(async move |this, cx| {
            let outcome = sync.await;

            this.update(cx, |this, cx| {
                this.subsonic_syncing = false;
                this.refresh_subsonic_rows(cx);

                if let (Err(e), Some(ix)) = (outcome, this.subsonic_index(id)) {
                    this.subsonic[ix].status = Some(rox_i18n::t!(
                        "settings-integrations-subsonic-sync-failed",
                        error = e
                    ));
                }

                cx.notify();
            })
            .ok();
        })
        .detach();

        // Otherwise a big library's line sits still for minutes and reads as
        // hung.
        cx.spawn(async move |this, cx| {
            while rox_services::sources::syncing() {
                cx.background_executor().timer(SUBSONIC_SYNC_POLL).await;

                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        })
        .detach();

        cx.notify();
    }
}

/// Most follows move nothing; a removal or a re-point does.
fn follow(this: WeakEntity<SettingsWindow>, pass: Task<usize>, cx: &mut App) -> Task<()> {
    cx.spawn(async move |cx| {
        pass.await;

        this.update(cx, |this, cx| {
            this.refresh_subsonic_rows(cx);
            cx.notify();
        })
        .ok();
    })
}

fn block_row(key: &'static str, control: impl IntoElement) -> Div {
    panel::setting_row(
        rox_i18n::t!(key),
        rox_i18n::try_translate(&format!("{key}.description")),
        control,
    )
}

/// Empty with no address or no sync yet. Its own connection, since the
/// catalog's belongs to the UI thread; a missing database is left alone, since
/// opening one creates it.
fn source_stats(library: &Entity<Library>, account: &SubsonicAccount, cx: &App) -> Stats {
    let Some(source) = rox_services::sources::source_of(account) else {
        return Stats::default();
    };

    let db = library.read(cx).db_path();
    if !db.exists() {
        return Stats::default();
    }

    rox_library::store::open(&db)
        .ok()
        .and_then(|conn| rox_library::store::stats_for_source(&conn, &source).ok())
        .unwrap_or_default()
}
