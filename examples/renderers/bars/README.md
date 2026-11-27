# Bars

An example rox renderer, written to be copied. It draws one bar for each signal in the
signal pool, a white stripe along the bottom that sweeps once every ten seconds of the
playing position and turns grey when the music stops, and a dot under the pointer that
turns solid while a button is held.
It's one Python file on nothing beyond the standard library.

Its manifest asks for the palette (`"palette": true` under `capabilities.renderer`), so
its enable card says it gets the colours of the current theme, and the bars take the
theme's accent from the first `palette` line rox sends. The background between them takes
the line's `stage`, the colour around the canvas's items, when rox sends one. Take that
line out of `plugin.json` and the bars keep their own four colours on their own
background.

It asks for the music's spectrum too (`"spectrum": true`), so its card says it gets the
music's spectrum, and it draws 64 thin bars along the top from the spectrum's log row,
each the louder of two neighbouring bins, falling back slowly when the music drops. Take
that line out and the row isn't drawn: rox lends no spectrum to a renderer that doesn't
ask.

It says `"resizes": true` too, so it reads the size to draw at from `hello` and from each
`size` line, and draws its next frame at that size into the start of its slot. Take that
line out and rox gives it one size for each session and fits the picture to the box.

The contract it follows is in [the renderers doc](../../../docs/03-implementation/11-renderers.md).

## Trying it

1. Press Reveal Folder on the Settings > Renderers page and copy this `bars` folder into
   it. Bars lists there, switched off.
2. Add a Renderer panel (Panels > Visualizers > Renderer), press Pick a Renderer and pick
   Bars. Its card comes up: Switch On confirms it. Switching Bars on from the Renderers
   page does the same.
3. Play something. Add signals under Application > Signals to get bars: each one in the
   pool is a bar.

A canvas takes it too: with Window > Design Mode on, right-click the empty stage, or
press the plus in the canvas inspector's header, and pick Add Render Target > Bars.

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
