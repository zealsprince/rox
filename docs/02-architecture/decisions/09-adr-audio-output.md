# ADR 9: Output layer stays swappable; bit-perfect is deferred

**Status:** Decided; deferral confirmed by product

Decision: abstract audio output behind an interface so a bit-perfect/exclusive-mode backend
can slot in later, but ship on cpal's shared-mode output. Don't build the exclusive path
now.

Alternatives: build per-platform exclusive output up front (WASAPI exclusive via the
`wasapi` crate, CoreAudio hog mode via `coreaudio-rs`, ALSA `hw` direct).

Trade: cpal can't do WASAPI exclusive or CoreAudio hog mode, so true bit-perfect output
means per-platform FFI beyond cpal, and DSD has no Rust decoder at all. The product spec
didn't ask for audiophile exclusive output, so building it now is speculative. Keeping the
output behind a trait costs little and leaves the door open. If bit-perfect becomes a real
requirement, that's a product decision that pulls this FFI work into scope.
