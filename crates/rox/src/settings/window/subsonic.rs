//! Subsonic servers on the Library page: a row each in the sources table
//! beside the folders, and a setup dialog behind the row for its switch,
//! address, login, Connect and Sync Now. `impl SettingsWindow` methods in a
//! child module, the way the workspace page is, with each server's state
//! beside them.
//!
//! A server's position in [`SettingsWindow::subsonic`] is its account's
//! index in accounts.json, and the two only move together: adding one
//! appends to both, removing one takes the same index out of both.
//! Everything that reaches back into the file from a callback finds its
//! index again by the server's id first, since one above it may have gone
//! in the meantime.

use super::*;

use gpui::Focusable;
use rox_core::settings::SubsonicAccount;

/// What the sources table answers to in the settings search on the
/// servers' behalf: the kinds of server, and the terms a server's own
/// settings carry. The dialog behind a row isn't searchable, so the table
/// has to be what a query for "password" finds.
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

/// How a field writes its value into the account it belongs to.
type Write = fn(&mut SubsonicAccount, String);

/// One server: its fields, written through per keystroke like the icecast
/// pair, and what this window has heard about the server.
pub(crate) struct SubsonicForm {
    /// Stable for as long as the window is open, unlike the server's
    /// position, which moves when one above it goes. What the element ids,
    /// the dialog and the remove confirm key the server by, and how a
    /// field's subscription finds its account again.
    id: u64,
    name: Entity<InputState>,
    url: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    /// The switch, copied from the file so the row renders without
    /// re-reading it.
    enabled: bool,
    /// Added from the menu and not confirmed yet. A fresh server stays out
    /// of the table, and cancelling its dialog takes it back out of the
    /// file, since it never held a row.
    fresh: bool,
    /// What the last Connect or Sync said about this server, already
    /// localized. None until one has run.
    status: Option<SharedString>,
    /// What the library holds under this server, and when it last synced.
    /// Read at open and again after anything that moves them, never per
    /// frame.
    stats: Stats,
    last_sync: i64,
    /// Whether a field has moved since the library last followed the
    /// accounts. The name counts too: it moves no rows, but the catalog
    /// reads server names on its next load, which the commit is.
    dirty: bool,
    _changes: Vec<Subscription>,
}

impl SubsonicForm {
    /// A server for `account`, its fields seeded from the file.
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
            // The password is stored exactly as typed. Trimming it the way
            // the others are would quietly break a login on a password that
            // really does end in a space.
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

    /// The account as the fields read right now, for the label and the
    /// source id the sync line is matched against.
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

    /// What the row and the dialog call the server: its name or host as
    /// typed so far, or New Server before there's either.
    fn label(&self, cx: &App) -> SharedString {
        let label = self.account(cx).label();
        if label.is_empty() {
            return rox_i18n::t!("settings-integrations-subsonic-new");
        }

        SharedString::from(label)
    }

    /// Whether an address has been typed, without which there's nothing to
    /// connect to, sync or add.
    fn addressed(&self, cx: &App) -> bool {
        !self.url.read(cx).value().trim().is_empty()
    }
}

impl SettingsWindow {
    /// The servers' rows in the sources table, under the folders. A row
    /// reads like a folder's, a name and its numbers, with where it stands
    /// beside the name: off, syncing, or when it last synced. The name opens
    /// the setup dialog, and so does the pencil beside the remove.
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
            // Named after the server, so its buttons are its own rather than
            // every other row's.
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

    /// Where a server stands, in the few words its row has room for: off,
    /// the album count while it syncs, or when it last did.
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

    /// Albums walked and albums to walk, when the running sync is this
    /// server's.
    fn subsonic_progress(&self, ix: usize, cx: &App) -> Option<(usize, usize)> {
        let source = rox_services::sources::syncing_source()?;
        let mine = rox_services::sources::source_of(&self.subsonic[ix].account(cx))?;

        (source == mine)
            .then(rox_services::sources::progress)
            .flatten()
    }

    /// The setup dialog for the server that's open, floated over the whole
    /// window like the confirm. A fresh server's asks to be added or
    /// cancelled; one already in the table's just closes, since every
    /// field has written through as it was typed. Escape answers the way
    /// the quiet button does.
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
                        // The page's own floor rather than the confirm's
                        // menu fill: this card holds the page's controls,
                        // and a switch's track is nearly the menu color, so
                        // on that fill only its knob shows.
                        .bg(palette::bg_root_opaque())
                        .border_1()
                        .border_color(palette::border_light())
                        .shadow_md()
                        .child(div().child(title))
                        .child(self.subsonic_fields(ix, cx))
                        .child(buttons),
                ),
        )
    }

    /// The dialog's body: the switch, and while it's on everything that
    /// reaches the server. A fresh server has no switch: it's being added
    /// to be used, and switching it off there would only fold the form
    /// away before there was anything to keep.
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
                    // A fresh server has nothing to sync until it's added,
                    // which starts the first sync itself.
                    .when(!form.fresh, |block| {
                        block.child(self.subsonic_sync_row(ix, cx))
                    })
            })
    }

    /// The connect strip, shaped like the scrobble destinations': what the
    /// last attempt said stands as the label, the button is the control.
    /// Connect with no URL typed would only ever fail, so it stays inert
    /// until there's a server to reach.
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

    /// The sync strip: where the library stands against this server on the
    /// left, the button that moves it on the right. Every other server's
    /// button waits while one syncs.
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

    /// What the server with this id is called, for the remove confirm to
    /// name the one it's about. New Server for one that's gone, which only
    /// a confirm left open across a removal could ask about.
    pub(super) fn subsonic_label(&self, id: u64, cx: &App) -> SharedString {
        match self.subsonic_index(id) {
            Some(ix) => self.subsonic[ix].label(cx),
            None => rox_i18n::t!("settings-integrations-subsonic-new"),
        }
    }

    /// Where the server with this id sits now, which is also its account's
    /// index in the file. None once it's been removed.
    fn subsonic_index(&self, id: u64) -> Option<usize> {
        self.subsonic.iter().position(|form| form.id == id)
    }

    /// What one of a server's fields just did. A change writes through on
    /// the keystroke; leaving the field is the commit, since the account
    /// may have moved and the library has to follow it: see
    /// `subsonic_moved` for why not on the keystroke.
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

    /// One keystroke's worth of a field, written through to its account and
    /// marked for the next commit. A field that moves where the server
    /// points also clears what the last Connect said, which was about the
    /// old server and stops standing for this one.
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

    /// The Add menu's Subsonic Server: a new account at the end of the file
    /// and its setup dialog, switched on so its fields show, with the
    /// address field focused since that's what it needs first. The server
    /// stays out of the table until the dialog's Add confirms it.
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

    /// Open a server's setup dialog from its row.
    fn open_subsonic(&mut self, id: u64, cx: &mut Context<Self>) {
        self.subsonic_editing = Some(id);
        cx.notify();
    }

    /// Close the dialog the quiet way: Cancel on a fresh server, Done or
    /// Escape on one already in the table. A fresh server goes back out of
    /// the file, since it never held a row and nobody said to keep it. One
    /// that stays gets the commit a left field would have run, since
    /// closing ends the edit without the field ever blurring.
    fn close_subsonic(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.subsonic_editing.take() else {
            return;
        };

        match self.subsonic_index(id) {
            // A running sync holds removals back, so the server can't go
            // yet. It joins the table instead, where its row can remove it
            // once the sync is done, rather than sitting in the file with
            // nothing on screen that owns it.
            Some(ix) if self.subsonic[ix].fresh && rox_services::sources::syncing() => {
                self.subsonic[ix].fresh = false;
            }

            Some(ix) if self.subsonic[ix].fresh => self.remove_subsonic(id, cx),

            Some(_) => self.subsonic_moved(cx),

            None => {}
        }

        cx.notify();
    }

    /// The dialog's Add, for a fresh server with an address typed: into the
    /// table, and straight into its first sync, since a server added to a
    /// library is a server whose catalog is wanted in it.
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

    /// Remove Server's yes, and a fresh server's Cancel: the account and its
    /// tracks go, and so does the server's state here. Refused while a sync
    /// runs, the same as the service refuses it: dropping the state without
    /// the account would leave every server after it pointed at the wrong
    /// account.
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

    /// A server's switch. The header table follows it, so a server pointed
    /// somewhere else and turned back on can authorize a stream without a
    /// restart, and so does the library: off takes the server's tracks out
    /// of every list without deleting them, on brings them back.
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

    /// A field was left, so follow the accounts with the authorize table
    /// and the library. The rows filed under an address an account has
    /// left can't be signed by anybody any more, so leaving them would show
    /// a shelf of tracks that skip themselves and blame the password.
    ///
    /// Leaving the field is the commit, the same one the broadcast rows
    /// take. Hanging this off the keystroke instead would drop the catalog
    /// on the first character typed, since mid-edit every character is its
    /// own source id, and pay for a projection rebuild on each one after
    /// it.
    ///
    /// Nothing here dials a server. A finished field is still just a field;
    /// Connect and Sync Now are the round trips.
    fn subsonic_moved(&mut self, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        if let Some(task) = self.subsonic_commit(this, cx) {
            self.subsonic_follow = Some(task);
        }
    }

    /// Follow the accounts the Subsonic fields now describe, if any of them
    /// has moved. `None` when none has, so a caller can tell a real pass
    /// from nothing to do.
    ///
    /// The window handle comes in weak and separate rather than off `cx`
    /// because the flush runs this while the entity is already on its way
    /// out; the numbers it would refresh have nowhere to land then, and
    /// the prune underneath still has to happen.
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

    /// Re-read every server's numbers and last sync off the file and the
    /// library. After anything that moves rows: a commit, a sync, a
    /// removal.
    fn refresh_subsonic_rows(&mut self, cx: &App) {
        let accounts = Settings::load().accounts.subsonic_servers;

        for (form, account) in self.subsonic.iter_mut().zip(&accounts) {
            form.stats = source_stats(&self.library, account, cx);
            form.last_sync = account.last_sync;
        }
    }

    /// Ping one server and keep what it answered for its status line. Off
    /// the UI thread, since it's a round trip to somebody's machine.
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
                    // A plain Subsonic server reports no name, so its
                    // protocol version is the most it can be called.
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

    /// Ask one server for its catalog and reconcile the library against
    /// it. Two tasks: one waits on the sync, the other keeps the album
    /// count moving on its row and in its dialog while it walks.
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

        // A big library is walked album by album, so without this the line
        // would sit still for minutes and read as hung.
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

/// Hold a pass the library runs to follow the accounts, and refresh every
/// server's numbers once it lands. Nothing moves on the overwhelming
/// majority of these, which is every edit that didn't change where an
/// account points, but a removal and a re-point both do.
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

/// A row inside the setup dialog, labelled and described off its message
/// key the way a section's keyed rows are.
fn block_row(key: &'static str, control: impl IntoElement) -> Div {
    panel::setting_row(
        rox_i18n::t!(key),
        rox_i18n::try_translate(&format!("{key}.description")),
        control,
    )
}

/// What the library holds under one server, the numbers a folder's row
/// shows. Empty while it names no address, or when its rows have never
/// synced. Its own connection rather than the catalog's, since the
/// catalog's belongs to the UI thread and this is one query a server, at
/// open and after a change. A database that isn't there is left alone for
/// [`StorageInfo::measure`]'s reason: opening one creates it.
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
