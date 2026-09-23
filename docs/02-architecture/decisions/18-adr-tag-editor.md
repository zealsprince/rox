# ADR 18: Tag editor as a shared batch form plus a per-file table

**Status:** Decided

Decision: the tag editor opens on a selection as one shared form covering the whole run.
A field that reads identically across every selected file shows that value. A field whose
values differ shows empty under a "multiple values" placeholder, so an empty box means
"these disagree" rather than "these are blank". Only fields the user actually changes write
anything, so opening the editor and closing it again is harmless.

Per-track fields, meaning title, track number, and disc number, lock while a batch is
selected. They have to, because a single form value written across the run would stamp
one title onto every file in it.

Fixing one file inside a batch is what table mode is for. The form swaps for a grid with
one row per track and a column per field, and the per-track fields the form had to lock
are editable in place there. Both views diff each file against that file's own baseline
and commit as a single batch through the writer's atomic layer
([ADR 4](04-adr-tagging.md)), so a field nobody touched is never rewritten, and a
successful commit updates the catalog directly without waiting for a rescan.

Alternatives: foobar's Edit Value dialog, which is the model this feature was first
specced against. There the shared form holds one field at a time, activating a field
steps into a per-file table showing just that field, and going back returns to the field
list with the field marked pending.

That was rejected on two counts. It edits one field at a time behind a modal, and it
needs a push/pop navigation with per-field pending state to track what's been changed but
not yet saved. A flat table shows every file and every per-track field at once instead.
The case that decides it is a messy import, where a run of tracks has titles and numbers
that all differ from each other: seeing the whole grid and tabbing across it is faster
than stepping into each field in turn, and it removes the pending-marker machinery
entirely rather than reimplementing it.

Trade: one shared set of pending edits backs both views, so an edit made in the form and
an edit made in a cell can target the same field, and something has to break the tie. The
rule is last edit wins, and it plays out in three places.

Entering the table folds a drifted form edit down into every cell the user hasn't already
touched, and stops treating it as form drift, so the form's value becomes the starting
point rather than a competing one. A cell the user had already moved keeps what it holds.
Leaving the table reads the cells back into the form, so a field that's now split across
files returns to the mixed placeholder. At save, a form field that's still armed is the
most recent typing and takes its whole column; otherwise each track's own cell wins.

The cost is that rule, plus giving up foobar's per-field pending affordance, which some
people will have muscle memory for. What it buys is a single grid that edits the whole
batch per file, in place.
