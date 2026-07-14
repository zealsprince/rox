//! The design system as data, in two halves panels pull the same way:
//! [`palette`] holds every color (ADR 10), [`tokens`] every shared size,
//! radius, and pace (ADR 12). Named decisions instead of inlined values.

pub mod palette;
pub mod tokens;
