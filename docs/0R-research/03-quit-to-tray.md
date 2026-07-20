# Quit to tray

Can rox keep playing with no windows open and come back through a tray icon?
This entry surveys what GPUI 0.2.2 and the crate ecosystem offer, and records
what the prototype (`crates/rox-prototype-tray`) found on the Plasma 6 Wayland
daily driver. Short version: the ksni side all works, but stock GPUI 0.2.2
kills the Linux event loop when the last window closes, so windowless
residency waits on the QuitMode policy upstream has already merged.

## Where the lifecycle stands

The app has two exits and they disagree. Quit (menu, cmd-q) persists and calls
`cx.quit()`, tearing down every window (`workspace.rs`). Closing the main
window's X only removes that window: the `on_window_should_close` hook persists
the layout and returns true, and nothing checks what's left. GPUI's Linux event
loop only exits on an explicit quit, so a settings window, popout, or customize
window keeps a headless process alive with no way back to a workspace. That's
today's bug, and it's worth fixing on its own before any tray work: when the
last workspace window closes and quit-to-tray is off, quit.

The same loop behavior is what background residency needs. The process already
survives with zero windows; what's missing is on purpose instead of by
accident, plus a handle back in.

## What GPUI gives us

Nothing tray-shaped in public API. The mac backend ships a `status_item.rs`
that wraps `NSStatusItem` as a platform window, but nothing exports it; it's
a Zed leftover, useful only as reference for a hand-rolled macOS item.

The lifecycle primitives are there, though:

- `App::on_window_closed` fires after any window closes. It doesn't say which,
  so telling "last workspace window" apart from "a popout" means counting our
  own windows, either a global registry of workspace handles or checking
  `cx.windows()` against the known secondary windows.
- `on_reopen` fires on macOS when the dock icon is clicked while the app runs
  with no windows. That's the reopen path there.
- `cx.open_window` works from a windowless app; the compositor connection
  outlives the windows. The prototype should confirm this on Wayland.

macOS barely needs a tray at all. GPUI doesn't implement
`applicationShouldTerminateAfterLastWindowClosed`, so the AppKit default
holds: the process stays, the dock icon stays, and `on_reopen` brings a
workspace back. That's the platform's native quit-to-tray. A menu bar status
item would be additive polish, not the mechanism.

Linux is where the work is, and Linux is the daily driver.

## Crate survey

Two real candidates, checked July 2026.

[ksni](https://docs.rs/ksni/latest/ksni/) 0.3.6, Linux only. A pure-Rust
implementation of the freedesktop StatusNotifierItem spec over D-Bus via zbus.
No GTK anywhere. The `blocking` feature runs the tray on its own thread, or it
rides an async runtime. The MPRIS plan (issue #25) already brings zbus into
the tree, so this is the same stack twice rather than a new one. The known
caveat: GNOME Shell doesn't render SNI items without the AppIndicator
extension. KDE and most other environments handle it natively. That caveat is
acceptable; it's the same one every SNI app carries.

[tray-icon](https://docs.rs/tray-icon/latest/tray_icon/) 0.24.1, the Tauri
project's cross-platform crate. On Linux it wants a GTK event loop running on
the tray's thread, which means pulling gtk3 and libappindicator as system
dependencies and running a dedicated `gtk::main()` thread next to GPUI's
calloop. On macOS it must create the icon on the main thread with the event
loop already running; that loop is GPUI's NSApp, so the interleaving is on us.
Cross-platform in name, but on Linux it's the heavier path for no gain over
ksni, and on macOS the dock already does the job.

Lean: ksni for Linux, AppKit defaults for macOS, revisit Windows when it
becomes a daily driver (an `NSStatusItem`-style story exists there through
either crate).

Either way the tray's callbacks land on the tray's own thread. Getting them
into GPUI means a channel drained by a task on the foreground executor, the
same marshalling MPRIS will need.

## What the tray is for

Less than it sounds. MPRIS already covers playback control from applets and
`playerctl` on Linux, so the tray's unique jobs are: show the app is alive,
reopen a workspace window, and quit. Play/pause and track skip in the tray
menu are cheap once the channel exists, but they're convenience, not the
point. The icon plus a three-item menu (open, play/pause, quit) is the whole
MVP.

## What the prototype answered

Ran July 2026 on Plasma 6 Wayland, KDE being SNI-native so no AppIndicator
extension in the way. The tray was driven over D-Bus (`busctl` on the item
and its dbusmenu, KWin scripting for window close), same events the shell
sends. `crates/rox-prototype-tray` is the harness: one GPUI window, ksni on
its blocking thread, an `async_channel` drained on the foreground executor,
a ticker thread standing in for the audio engine.

The blocker first: **stock GPUI 0.2.2 cannot run windowless on Linux.** Both
backends stop the event loop when the last window drops
(`wayland/client.rs:387` and `x11/client.rs:250`,
`if state.windows.is_empty() { signal.stop() }`), so the process exits
cleanly out from under the tray, the drain loop, and the music. The research
framing above ("the process already survives with zero windows") only held
because a secondary window was still open. Upstream has already fixed this
properly: zed PR #42391 (merged 2025-11-10, three weeks after 0.2.2's
publish) moved the decision to an app-level policy,
`Application::with_quit_mode`, where `QuitMode::Explicit` is exactly what
rox wants since the workspace already counts windows and quits itself. No
crates.io release carries it yet. The implementation therefore waits on a
gpui bump, or ships with the backend check patched out (two lines per
backend); the prototype's windowless runs used exactly that patch against
the 0.2.2 source and nothing else.

With the patch in, every question came back yes:

- The round trip holds. Menu click lands on ksni's service thread, the
  activate closure does a non-blocking `try_send`, the drain loop flips app
  state and pushes it back with `Handle::update`, and the menu label reads
  Pause or Play to match. `Handle::update` blocks its caller until the
  service thread acks; called from the foreground executor it returns
  effectively instantly and cannot deadlock as long as menu closures never
  call back into GPUI, which the channel design guarantees.
- A windowless app reopens fine. `cx.open_window` from the drain loop puts a
  window up, and KWin focuses it without any xdg-activation work on our side.
  Other compositors may apply focus-stealing prevention here; that stays a
  per-compositor observation, not something we can force.
- Windowless idle is actually zero: 0 jiffies over a 6 second sample while
  the ticker thread kept "playing". The per-window notify loop dies with the
  view, so nothing repaints and nothing wakes.
- Quit from the tray is clean. `handle.shutdown().wait()` then `cx.quit()`
  exits inside 200 ms with the D-Bus name released, windows or none.

Smaller findings worth keeping:

- `App::on_window_closed` fires reliably on the compositor close path, so
  counting workspace windows there is sound for the real implementation.
- The Wayland backend ignores `TitlebarOptions.title` at 0.2.2; the caption
  comes up empty until `set_window_title` is called. rox already sets titles
  at runtime, but anything matching windows by caption (scripts, tests)
  should match on app id instead.
- The 2048 px app icon is 16 MB as an SNI pixmap; thumbnail to 64 px before
  handing it to ksni.
- The GNOME caveat stands as written: SNI works untouched on KDE, GNOME still
  needs the AppIndicator extension. Nothing new to add from this run.

## macOS and Windows

Surveyed from the 0.2.2 source and upstream, not run on hardware; nothing
here needed a prototype yet.

macOS confirms the survey's read. The 0.2.2 mac backend registers no
`applicationShouldTerminateAfterLastWindowClosed`, so the AppKit default
holds: the process and dock icon outlive the last window, and `on_reopen`
(wired to `applicationShouldHandleReopen`) brings a workspace back on dock
click. Upstream's `QuitMode` keeps Explicit as the mac default, so nothing
changes on a bump. Quit-to-tray there is just close_workspace_window not
calling `cx.quit` when the toggle is on; the dock is the tray, no crate
involved. The mac backend's `status_item.rs` turns out to be dead gpui1-era
code - it imports `geometry::rect`, which no longer exists, and isn't in the
module tree - so a menu bar status item would be hand-rolled NSStatusItem
via objc, or tray-icon on the main thread (GPUI's main thread is the running
NSApp loop it wants). Still additive polish, not the mechanism.

Windows has the same 0.2.2 disease as Linux: `WM_GPUI_CLOSE_ONE_WINDOW`
posts `WM_QUIT` once the window list empties
(`platform/windows/platform.rs:718`), and upstream removed that in the same
QuitMode work, so the one gpui bump unlocks all three platforms. For the
icon, tray-icon is the lean there, opposite of Linux: no GTK anywhere on
Windows, just a thin wrapper over `Shell_NotifyIcon`, and its docs
explicitly support a dedicated thread running its own win32 message pump.
That is the ksni architecture again - tray thread, non-blocking sends into a
channel, drained on the foreground executor - so the marshalling layer from
the prototype carries over unchanged. Hand-rolling over the `windows` crate
gpui already pulls in stays the fallback if tray-icon's menu stack
(muda) fights the win32 loop. souvlaki's SMTC also wants the window handle
wired up (`media_controls.rs` notes it) before the media widget works there;
that stays a separate ticket either way.
