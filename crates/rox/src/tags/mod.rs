//! Tag editing surfaces: the multi-file tag editor, the online tag
//! matcher, the filename pattern guesser, the batch repair pass, and the
//! field-completion suggester.

pub mod editor;
pub mod guess;
pub mod matcher;
pub mod repair;

// The field-completion provider moved to rox-panel-api so the search panel
// could take it along; it answers to the path the tag editor always used.
pub use rox_panel_api::suggest;
