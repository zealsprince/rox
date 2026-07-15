# Quit to tray

Can rox keep playing with no windows open and come back through a tray icon?
This entry surveys what GPUI 0.2.2 and the crate ecosystem offer and frames the
prototype that has to run before the feature gets built. No decision leans on
this yet.

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

## What the prototype has to answer

- Does ksni's thread plus a channel into GPUI's foreground executor hold up:
  clicks arrive, menu state (play/pause label) updates from app state, nothing
  deadlocks at quit.
- Can a windowless GPUI app on Wayland open a new workspace window from a tray
  click, and does it get focus (xdg-activation is the compositor-side wrinkle).
- Does the windowless process idle clean: no repaint loop, no CPU burn, audio
  keeps playing.
- What quitting looks like when the tray owns the process: the tray thread has
  to shut down without hanging the D-Bus connection.
