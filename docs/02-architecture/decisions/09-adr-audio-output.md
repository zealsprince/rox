# ADR 9: Output layer stays swappable; bit-perfect is deferred

**Status:** Decided; deferral confirmed by product

Decision: put audio output behind an interface so a bit-perfect, exclusive-mode backend
can slot in later, and ship on cpal's shared-mode output for now. Don't build the
exclusive path yet.

Alternatives: build per-platform exclusive output up front (WASAPI exclusive via the
`wasapi` crate, CoreAudio hog mode via `coreaudio-rs`, ALSA `hw` direct).

Trade: what "bit-perfect" needs is a stream the OS mixer doesn't touch, and cpal doesn't
offer one on any platform. It can't open WASAPI in exclusive mode or take a CoreAudio
device in hog mode, so getting there means writing per-platform FFI underneath cpal
rather than configuring it differently. DSD, which is the other thing audiophile output
usually implies, has no Rust decoder at all, so that half isn't a matter of effort.

The product spec never asked for exclusive output, so building it now would be guessing
at a requirement. Keeping output behind a trait costs almost nothing today and leaves
the door open. That's the cheap half of the work. The expensive half is the FFI, and it's
only worth spending when someone actually wants what it buys. If bit-perfect
does become a real requirement, that's a product decision, and it pulls the FFI work
into scope with it.

**Amendment:** that product decision happened (#70). The deferral ends with
[ADR 19](19-adr-processing-chain.md), which defines the backend contract, the exclusive
mode, and the bypass rule that makes bit-perfect a claim you can check rather than a
label.
