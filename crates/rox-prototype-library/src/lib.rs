//! Research prototype for the library scale question: ADR 5 (SQLite source of
//! truth plus a full in-memory columnar projection) and ADR 6 (in-memory
//! substring search) were sized against 50-100k tracks. Does the same shape
//! hold at 10 million - cold open, projection RAM, sub-second search, instant
//! sort and filter, scroll windows - and where does it have to split across
//! cores to get there?
//!
//! The store and projection under test graduated into rox-library; this crate
//! keeps the deterministic generator and the timing harness that exercised
//! them. No real files are involved. Run with --release; see the crate binary
//! for the harness.

pub mod gen;
