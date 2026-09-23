# ADR 13: Panel theming: a sparse override scope on the palette read path

**Status:** Decided

Decision: a panel can carry its own look as a `PanelTheme`. That's a sparse map of
palette-role overrides, an optional surface opacity, and the frame knobs: a margin
between the panel and its cell with the backdrop showing through the gap, padding inside
the panel's own surface, corner rounding, and a border width. It's stored on the panel's
config and persisted through the layout dump like every other per-view setting.

Margin, padding, and border each hold a value per side, but a knob whose four sides
agree is written as a single number. So the everyday case is still one slider, and a
config saved before per-side values existed reads back exactly as it did before.

The frame knobs are geometry rather than color, so the wrapper element applies them
directly to the panel body instead of routing them through the theme scope below. The
one exception is the border, which draws in the border role's color, and that role is
already in the override map like any other.

**How a panel's colors reach its drawing.** The read path stays the plain accessors of
[ADR 10](10-adr-theming.md), with no signature changes anywhere. While a panel renders,
a thread-local scope stack holds its resolved theme. Each accessor checks that scope
first and returns the panel's value if the role is overridden there, and otherwise falls
through to the process-global palette. A wrapper element pushes the scope for the render
build and pushes it again for layout, prepaint, and paint, because that's when hover
styles and canvas paint closures actually call the accessors, and the render build is
long over by then. Panel code keeps calling `palette::accent()` and gets its own accent,
without a context parameter threaded through anything.

An overridden role reads exactly as it was written. Song theming and palette easing move
right past it, so a pinned role stays where it was pinned. Every role the panel doesn't
override keeps following the app palette, including live edits and cover-art tinting.

**Editing.** A panel gets its own settings window, built on the same sidebar-and-pages
shape as the app settings window. The panel's old customize rows become pages of its
own, and a shared Appearance page edits the override; both draw from one chrome module
extracted for the purpose. A panel can also append a section to that Appearance page for
look knobs it stores on its config rather than in the theme, the way the grid does for
cover rounding.

Alternatives: threading a palette handle through every accessor call site; giving each
panel a full palette instead of a sparse diff; running per-panel overrides through the
derivation and easing pipeline so they get full song-theming parity; adding per-panel
overrides for the ADR 12 tokens (size, radius, pace) in the same stroke.

**Trade.** The scope is invisible in the accessor signatures, which is the whole point
and also the cost: a paint that escapes the wrapped subtree, like a deferred overlay or
a menu, silently reads the global palette instead of the panel's. That's accepted
because overlays are app chrome rather than panel content, so the global palette is the
right answer for them anyway.

A sparse diff means an override keeps tracking global edits for every role it doesn't
pin, where a full per-panel copy would freeze the panel against them the moment it was
made. What it gives up is that a panel theme isn't a standalone palette file someone
can hand around on its own.

Overridden roles beat song theming because the reason to pin a role is to have it stay
put while the rest of the app moves. Skipping derivation for those roles also keeps the
scope a read-time lookup rather than a second derivation pipeline running beside the
first.

gpui-component's widget chrome, meaning table striping and
tab bars, projects from the global theme only, so a panel override recolors the panel's
own drawing while the widget skeleton underneath stays on the app palette. Rounding also
styles the body's own background quad rather than clipping to it, because gpui's content
masks are rectangular: content pushed hard into a rounded corner still paints square.
Small radii keep that invisible. Covers are the exception, since they run edge to edge
with nothing to hide behind, so the art surfaces round their images themselves.

Two smaller things fell out along the way. The border's old per-side on/off mask folded
into the widths, since a side set to zero draws nothing. One control now does what two
used to. Configs holding the old mask still load, and the mask folds over whichever width
wins until the border is next edited. And tokens stay ADR 12 consts, because the frame knobs
shape the panel's outer edge while the tokens govern the spacing and radii of what's
inside it, which are different questions with different owners.
