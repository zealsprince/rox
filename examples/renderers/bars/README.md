# Bars

An example rox renderer, written to be copied. It draws one bar for each signal in the
signal pool, a white stripe along the bottom that sweeps once every ten seconds of the
playing position, and a dot under the pointer that turns solid while a button is held.
It's one Python file on nothing beyond the standard library.

Its manifest asks for the palette (`"palette": true` under `capabilities.renderer`), so
its enable card says it gets the colours of the current theme, and the bars take the
theme's accent from the first `palette` line rox sends. The background between them takes
the line's `stage`, the colour around the canvas's items, when rox sends one. Take that
line out of `plugin.json` and the bars keep their own four colours on their own
background.

The contract it follows is in [the renderers doc](../../../docs/03-implementation/11-renderers.md).

## Trying it

1. Turn on Enable Plugins at the top of Settings > Plugins.
2. Press Reveal Folder on the Plugins page and copy this `bars` folder into it. Bars
   lists under Renderers, switched off.
3. Switch Bars on and confirm the card.
4. Add a Canvas panel (Panels > Arrangement > Canvas). With Panels > Design Mode on,
   right-click the empty stage, or press the plus in the canvas inspector's header, and
   pick Add Render Target > Bars.
5. Leave design mode and play something. Add signals under Application > Signals to get
   bars: each one in the pool is a bar.

Python 3.7 or later has to be on PATH as `python3` or `python`, or as `py` on Windows.
The example runs on Linux and macOS. On Windows rox hands a renderer a section handle
rather than a file descriptor, and Python's `mmap` can't map one, so it logs that and
exits.

## Making it yours

- Turn on Developer mode, the terminal button beside the renderer's switch, while you
  work on it. Otherwise every save switches it off until you approve it again. A running
  renderer keeps the code it started with: switch it off and on to start the new code.
- Change `id` in `plugin.json` and rename the folder to match.
- Keep the counter: odd before the pixels go in, even after. rox drops a frame whose
  counter is odd or moved while it copied, and refuses one whose size words don't match
  the target. Sixty refused frames in a row stop the renderer.
- Keep honouring `pause`. While paused rox doesn't read stdout, so a renderer that goes
  on announcing frames fills the pipe and blocks.
- Keep exiting when stdin closes. That's how rox asks a renderer to go when a target is
  deleted or resized, and it kills one that's still there 200 ms later.
- Print announce lines and nothing else on stdout. Write anything for a person to stderr,
  which goes to rox's log.
