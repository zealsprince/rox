# Open questions

Undecided items that need answering before or during implementation. Resolved items are
removed once their resolution lands in the spec; the commit history is the record.

1. **Extension host mechanism** - sources ship as community extensions
   ([scope](01-product/03-scope.md#sources-as-extensions)). Zed's answer on the same UI
   stack is WASM (wasmtime + WIT), which sandboxes cleanly, but it's unclear whether
   audio decode and decryption (librespot-class work) fits in WASM or whether
   audio-capable sources need a subprocess model. Needs a research pass and a prototype
   before an ADR. Nothing in the core blocks on this.
2. **Unified library matching** - merging the same album across local and streaming
   sources needs entity resolution (ISRC, tag matching), and it's unclear how far that
   gets in practice. Source-qualified track identity keeps the door open; the matching
   question itself waits until a second source exists.
