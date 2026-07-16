# ADR 13: Panel theming: a sparse override scope on the palette read path

**Status:** Decided

Decision: a panel can carry its own look as a `PanelTheme`, a sparse map of
palette-role overrides plus an optional surface opacity, stored on the
panel's config and persisted through the layout dump like every other
per-view knob. The read path stays the plain accessors of ADR 10: while a
panel renders, a thread-local scope stack holds its resolved theme, and each
accessor answers with the scope's value for an overridden role before
falling through to the process-global palette. A wrapper element pushes the
scope for the render build and re-enters it for layout, prepaint, and paint,
which is when hover styles and canvas paint closures actually read - so
panel code keeps calling `palette::accent()` with no context threading. An
overridden role reads exactly as written: song theming and palette easing
pass it by, while every role the panel leaves alone keeps following the app
palette, edits and tinting included. Editing lives in a per-panel settings
window with the app settings window's sidebar-and-pages shape; the panel's
old customize rows become its own pages and a shared Appearance page edits
the override, both drawing from one extracted chrome module.

Alternatives: threading a palette handle through every accessor call site;
a full palette per panel instead of a sparse diff; running per-panel
overrides through the derivation and easing pipeline for full song-theming
parity; per-panel token (size, radius, pace) overrides in the same stroke.

Trade: the scope is invisible in signatures, so a paint that escapes the
wrapped subtree (a deferred overlay, a menu) silently reads the global
palette - accepted because overlays are app chrome, not panel content. A
sparse diff keeps an override tracking global edits for everything it does
not pin, where a full copy would freeze the panel against them; the cost is
that a theme is not a standalone palette file. Overridden roles winning over
song theming is the point - a pinned role holds still while the app moves -
and skipping derivation for them keeps the scope a read-time lookup instead
of a second derivation pipeline. gpui-component widget chrome (table
striping, tab bars) projects from the global theme only, so a panel override
recolors the panel's own drawing, not the widget skeleton under it. Tokens
stay ADR 12 consts; if a panel ever needs its own spacing, the scope shape
is sitting right there.
