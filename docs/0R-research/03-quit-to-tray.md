# Quit to tray

Can rox keep playing with no windows open and come back through a tray icon?
This entry surveys what gpui 0.2.2 and the crate ecosystem offer, and records
what the prototype found on the Plasma 6 Wayland daily driver. The answer was
yes on every question except one: stock gpui 0.2.2 kills the Linux event loop
when the last window closes. [What shipped](#what-shipped) records how that
was resolved.

The prototype was in `crates/rox-prototype-tray` (git history).

## Where the lifecycle stood

The app had two exits and they disagreed. Quit (menu, cmd-q) persists and calls
`cx.quit()`, tearing down every window (`workspace.rs`). Closing the main
window's X only removed that window: the `on_window_should_close` hook persists
the layout and returns true, and nothing checked what was left. gpui's Linux event
loop only exits on an explicit quit, so a settings window, popout, or customize
window kept a headless process alive with no way back to a workspace. That was a
bug worth fixing on its own before any tray work: when the last workspace window
closes and quit-to-tray is off, quit.

That same loop behavior is the one background residency needs. The process already
survived with zero windows; what was missing was making that a choice rather than
an accident, plus a handle back in.

## What gpui gives us

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

macOS barely needs a tray at all. gpui doesn't implement
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
runs on an async runtime. The MPRIS plan (issue #25) already brings zbus into
the tree, so this is the same stack twice rather than a new one. The known
caveat: GNOME Shell doesn't render SNI items without the AppIndicator
extension. KDE and most other environments handle it natively. That caveat is
acceptable; it's the same one every SNI app has.

[tray-icon](https://docs.rs/tray-icon/latest/tray_icon/) 0.24.1, the Tauri
project's cross-platform crate. On Linux it wants a GTK event loop running on
the tray's thread, which means pulling gtk3 and libappindicator as system
dependencies and running a dedicated `gtk::main()` thread next to gpui's
calloop. On macOS it must create the icon on the main thread with the event
loop already running; that loop is gpui's NSApp, so the interleaving is on us.
Cross-platform in name, but on Linux it's the heavier path for no gain over
ksni, and on macOS the dock already does the job.

Lean: ksni for Linux, AppKit defaults for macOS, revisit Windows when it
becomes a daily driver (an `NSStatusItem`-style story exists there through
either crate).

Either way the tray's callbacks run on the tray's own thread. Getting them
into gpui means a channel drained by a task on the foreground executor, the
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
sends. `crates/rox-prototype-tray` is the harness: one gpui window, ksni on
its blocking thread, an `async_channel` drained on the foreground executor,
a ticker thread standing in for the audio engine.

The blocker first: **stock gpui 0.2.2 cannot run windowless on Linux.** Both
backends stop the event loop when the last window drops
(`wayland/client.rs:387` and `x11/client.rs:250`,
`if state.windows.is_empty() { signal.stop() }`), so the process exits
cleanly out from under the tray, the drain loop, and the music. The research
framing above ("the process already survives with zero windows") only held
because a secondary window was still open. Upstream has already fixed this
properly: zed PR #42391 (merged 2025-11-10, three weeks after 0.2.2's
publish) moved the decision to an app-level policy,
`Application::with_quit_mode`, where `QuitMode::Explicit` is what
rox wants since the workspace already counts windows and quits itself. No
crates.io release carried it at the time, leaving two routes: wait for a gpui
bump, or patch the backend check out, two lines per backend. The prototype's
windowless runs used that patch against the 0.2.2 source and nothing
else.

With the patch in, every question came back yes:

- The round trip holds. Menu click arrives on ksni's service thread, the
  activate closure does a non-blocking `try_send`, the drain loop flips app
  state and pushes it back with `Handle::update`, and the menu label reads
  Pause or Play to match. `Handle::update` blocks its caller until the
  service thread acks; called from the foreground executor it returns
  effectively instantly and cannot deadlock as long as menu closures never
  call back into gpui, which the channel design guarantees.
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
code. It imports `geometry::rect`, which no longer exists, and isn't in the
module tree, so a menu bar status item would be hand-rolled NSStatusItem
via objc, or tray-icon on the main thread (gpui's main thread is the running
NSApp loop it wants). Still additive polish, not the mechanism.

Windows has the same 0.2.2 disease as Linux: `WM_gpui_CLOSE_ONE_WINDOW`
posts `WM_QUIT` once the window list empties
(`platform/windows/platform.rs:718`), and upstream removed that in the same
QuitMode work, so one fix unlocks all three platforms. For the icon,
tray-icon is the lean there, opposite of Linux: no GTK anywhere on
Windows, just a thin wrapper over `Shell_NotifyIcon`, and its docs
explicitly support a dedicated thread running its own win32 message pump.
That's the ksni architecture again, a tray thread doing non-blocking sends into
a channel drained on the foreground executor, so the marshalling layer from
the prototype carries over unchanged. Hand-rolling over the `windows` crate
gpui already pulls in stays the fallback if tray-icon's menu stack
(muda) fights the win32 loop. souvlaki's SMTC also needs the window handle
wired up (`media_controls.rs` notes it) before the media widget works there;
that stays a separate ticket either way.

## What shipped

The event-loop blocker was resolved by patching the vendored gpui rather than
waiting for a release, the same custody the shader work already needs:
`patches/gpui/quit-keep-event-loop.patch` drops the last-window loop stop from
the Wayland, X11, and Windows backends, with a note to delete the patch once a
crates.io gpui ships `with_quit_mode`. rox already quits itself from
`close_workspace_window`, so removing the auto-stop is the whole change.

The tray itself is `crates/rox/src/integrations/tray.rs`, and it took the
survey's lean on every platform: ksni on Linux, tray-icon on Windows, and
nothing on macOS beyond the kept state, since the dock is the tray there. The
marshalling matches `media_controls`, callbacks on the tray's own thread
sending over a channel that a foreground-executor task drains.

Two things the prototype didn't cover came up in the build. Closing the last
window hands the live `AppState` to a hold rather than dropping it, so the
player and its engine keep running with no window attached and the tray's Open
adopts that state into a fresh one. And residency is checked, not assumed: on
Linux the close path only goes windowless if the icon actually made it onto the
bus, so a desktop with no SNI host quits instead of stranding a headless
process.
