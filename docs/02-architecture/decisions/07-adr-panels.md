# ADR 7: Panel/dock: build on gpui primitives, adopt gpui-component as the widget baseline

**Status:** Decided

Decision: adopt `gpui-component` (longbridge) for the heavy widgets it already solves,
meaning virtualized tables, image elements, and its dock. Build our own layer for the two
things it doesn't give us: duplicating a panel with its own config, and popping a panel
out into an OS window.

Alternatives: vendor and strip down Zed's `workspace::dock`, or roll the entire panel
system on raw gpui primitives.

Trade: Zed's dock is proven, but it's built around Zed's own `workspace` concept and
arrives with editor assumptions attached, so taking it means untangling it from all of
that first. Rolling the whole system on raw primitives is the other end: total control,
and every widget from scratch, including the virtualized table that the library view
lives or dies on.

`gpui-component` is permissively licensed, actively maintained, and built for
this kind of app, so it removes most of the widget work in one step. Two things come
with it. It's a second pre-1.0 dependency, and it tracks gpui's churn, so it inherits
the pinning discipline ADR 1 already imposes. And its dock only manages panels inside
one window, which doesn't cover pop-out.

Pop-out doesn't argue against it, though, because pop-out isn't a dock feature under
any of these options. It's a separate mechanism, gpui's multi-window support with
entities shared between the windows, so we're building it ourselves whichever dock we
pick. That's what settles the decision: take the acceleration on everything the library
already solves, own the two panel behaviors that are the product, and stay ready to
vendor the dock if we outgrow it.

**Amendment: the escape hatch is exercised.** Three dock behaviors we wanted next had no
upstream hook to reach them through: suppressing the tab bar when a group holds only one
panel, closing a whole tab on middle-click, and clearing the zoom flag when it goes
stale. So the dock is vendored as `rox-dock`, which is gpui-component 0.5.1's `dock`
module plus the three modules it reaches into through `pub(crate)` coupling
(`resizable`, `tab`, and `history`), under their Apache-2.0 license. Everything else,
widgets and theme included, still comes from the published crate at the pinned version.

The alternative was forking the whole crate through `[patch.crates-io]`. Vendoring one
leaf module instead keeps custody scoped to the code we actually change, so the rest
keeps updating normally. The new cost comes at gpui-component bumps, where `rox-dock`
has to be re-diffed against upstream's `src/dock` as part of that budgeted task.
