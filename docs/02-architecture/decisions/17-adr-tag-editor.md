# ADR 17: Tag editor as a shared batch form plus a per-file table

**Status:** Decided

Decision: the tag editor opens on a selection as one shared form over the whole run. A
field every file agrees on shows its value, differing values show empty under a "multiple
values" placeholder, and only fields the user moves write anything. Per-track fields
(title, track number, disc number) lock in a batch, since one form value would stamp the
same title over every file. To fix a single file inside a batch, the form swaps for a
table: one row per track, columns for every field, the per-track fields a batch has to
lock editable in place. Both views diff per file against each file's own baseline and
commit as one batch through the writer's atomic layer ([ADR 4](04-adr-tagging.md)), so an
unchanged field never rewrites and a success lands in the catalog without a rescan.

The per-file need, fix one file's value without collapsing the selection, is served by
table mode, not by a per-field step-in.

Alternatives: foobar's Edit Value dialog, the model the feature was first specced against.
There the shared form holds one field at a time, and activating a field steps into a
per-file table of just that field, with back returning to the field list and marking the
field pending. Rejected because it edits one field behind a modal and needs a push/pop
with per-field pending state, where the flat table shows every file and every per-track
field at once. For the messy-import case, correcting a run of tracks whose titles and
numbers all differ, seeing the whole grid and tabbing through it is faster than stepping
into each field in turn, and it drops the pending-marker machinery entirely.

Trade: one shared pending set feeds two views, so form edits and cell edits to the same
field need a last-edit-wins rule. Entering the table folds a drifted form edit into every
untouched cell and stops counting it as form drift; a cell the user already moved keeps its
value; leaving the table re-reads the cells back into the form, so a split field goes back
to the mixed placeholder. At save, an armed form field is the newest typing and wins its
column, otherwise each track's own cell speaks. The cost is that rule and the loss of
foobar's exact per-field pending affordance; the gain is one grid that edits the whole
batch, per-file, in place.
