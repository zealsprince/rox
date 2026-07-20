//! Tag value suggestions: a completion provider over the library's
//! distinct values for one field, for any input editing that field. The
//! menu is the input widget's own, so arrows and enter come with it;
//! accepting an item replaces the whole input through the item's text
//! edit, so multi-word values land whole even from a mid-word match.
//! Attach through [`provider`] wherever a tag field gets typed.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{Context, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope, RopeExt as _};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, Position, TextEdit,
};

use rox_library::projection::{Projection, QueryField, SymTable, QUERY_FIELDS};
use rox_library::writer::Field;

/// How many suggestions the completion menu shows at once.
const CAP: usize = 20;

/// The byte length of `label`'s leading chars whose case-fold matches
/// `typed`, 0 for a non-prefix match. Feeds each item's filter_text: the
/// menu highlights that many bytes of the label, and its fallback - the
/// raw typed token - lands mid-char or past the end on short and
/// non-ascii labels, tripping gpui's char boundary assert.
fn matched_prefix_len(label: &str, typed: &str) -> usize {
    if typed.is_empty() {
        return 0;
    }
    let mut lower = String::new();
    for (i, c) in label.char_indices() {
        lower.extend(c.to_lowercase());
        if lower.len() >= typed.len() {
            return if lower.starts_with(typed) {
                i + c.len_utf8()
            } else {
                0
            };
        }
        if !typed.starts_with(&lower) {
            return 0;
        }
    }
    // The label ran out first: typed is longer than the label.
    0
}

/// A table's values matching `typed`, case-folded, prefix matches first,
/// at most [`CAP`]. An empty `typed` lists the table from the top.
fn ranked<'a>(table: &'a SymTable, typed: &str) -> Vec<&'a String> {
    let mut prefixed = Vec::new();
    let mut contained = Vec::new();
    for (value, lower) in table.strings.iter().zip(&table.lower) {
        if value.is_empty() {
            continue;
        }
        if lower.starts_with(typed) {
            prefixed.push(value);
            if prefixed.len() >= CAP {
                break;
            }
        } else if contained.len() < CAP && lower.contains(typed) {
            contained.push(value);
        }
    }
    prefixed.extend(contained);
    prefixed.truncate(CAP);
    prefixed
}

/// The provider for `field`, when it is a name field whose values recur
/// across a library and there is a projection to draw them from. Free
/// text and numeric fields get none.
pub fn provider(
    projection: Option<&Arc<Projection>>,
    field: &Field,
) -> Option<Rc<dyn CompletionProvider>> {
    if !matches!(
        field,
        Field::Artist | Field::AlbumArtist | Field::Album | Field::Genre
    ) {
        return None;
    }
    Some(Rc::new(FieldSuggestions {
        projection: projection?.clone(),
        field: field.clone(),
    }))
}

/// One field's suggestion source: the projection's interned distinct
/// values, shared with the library they came from. Typing filters them
/// case-folded the way the library search does, prefix matches first.
struct FieldSuggestions {
    projection: Arc<Projection>,
    field: Field,
}

impl FieldSuggestions {
    fn table(&self) -> &SymTable {
        match self.field {
            Field::Artist => &self.projection.artists,
            Field::AlbumArtist => &self.projection.album_artists,
            Field::Album => &self.projection.albums,
            _ => &self.projection.genres,
        }
    }
}

impl CompletionProvider for FieldSuggestions {
    fn completions(
        &self,
        text: &Rope,
        _offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let typed = text.to_string().to_lowercase();
        // An emptied input closes the menu instead of listing everything.
        if typed.is_empty() {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        }
        let whole = lsp_types::Range::new(Position::new(0, 0), text.offset_to_position(text.len()));
        let items = ranked(self.table(), &typed)
            .into_iter()
            .map(|value| CompletionItem {
                label: value.clone(),
                filter_text: Some(value[..matched_prefix_len(value, &typed)].to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: whole,
                    new_text: value.clone(),
                })),
                ..Default::default()
            })
            .collect();
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        _new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        // Every keystroke requeries - deletions too, so the list follows
        // shrinking text and an emptied field closes the menu.
        // Programmatic fills go through the silent path and never reach
        // this.
        true
    }
}

/// The provider for a search box speaking the query syntax: values for
/// the `field:` term under the cursor, drawn from that field's table,
/// and the field prefixes themselves for a bare word that starts one.
/// Anything else gets no menu, so plain title searches stay quiet.
pub fn query_provider(projection: Option<&Arc<Projection>>) -> Option<Rc<dyn CompletionProvider>> {
    Some(Rc::new(QuerySuggestions {
        projection: projection?.clone(),
    }))
}

struct QuerySuggestions {
    projection: Arc<Projection>,
}

/// The span of the query token covering `offset`. Tokens split on
/// whitespace outside double quotes, same as the projection's parser;
/// a cursor in the gaps has no token.
fn token_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let mut start = None;
    let mut in_quotes = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if let Some(s) = start.take() {
                    if (s..=i).contains(&offset) {
                        return Some((s, i));
                    }
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(i);
    }
    let s = start?;
    (s <= offset).then_some((s, text.len()))
}

/// The field a raw token pins and the offset its value starts at within
/// the token, for a token with a known unquoted `field:` prefix.
fn field_term(raw: &str) -> Option<(QueryField, usize)> {
    let colon = raw.find(':')?;
    let name = &raw[..colon];
    if name.contains('"') {
        return None;
    }
    let name = name.to_lowercase();
    let (_, field) = QUERY_FIELDS.iter().find(|(n, _)| *n == name)?;
    Some((*field, colon + 1))
}

impl CompletionProvider for QuerySuggestions {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        _cx: &mut Context<InputState>,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let none = || Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        let string = text.to_string();
        let Some((start, end)) = token_at(&string, offset.min(string.len())) else {
            return none();
        };
        let raw = &string[start..end];
        let strip = |s: &str| -> String {
            s.chars()
                .filter(|&c| c != '"')
                .collect::<String>()
                .to_lowercase()
        };
        let items = if let Some((field, value)) = field_term(raw) {
            let table = match field {
                QueryField::Artist => &self.projection.artists,
                QueryField::AlbumArtist => &self.projection.album_artists,
                QueryField::Album => &self.projection.albums,
                QueryField::Genre => &self.projection.genres,
                // Free text and numbers have no table to suggest from.
                QueryField::Title | QueryField::Year => return none(),
            };
            let typed = strip(&raw[value..]);
            // Accepting rewrites the whole value span, quoted when the
            // value has spaces so it survives the tokenizer.
            let span = lsp_types::Range::new(
                text.offset_to_position(start + value),
                text.offset_to_position(end),
            );
            ranked(table, &typed)
                .into_iter()
                .map(|value| {
                    let quoted = if value.chars().any(char::is_whitespace) {
                        format!("\"{value}\"")
                    } else {
                        value.clone()
                    };
                    CompletionItem {
                        label: value.clone(),
                        filter_text: Some(value[..matched_prefix_len(value, &typed)].to_string()),
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range: span,
                            new_text: quoted,
                        })),
                        ..Default::default()
                    }
                })
                .collect()
        } else {
            // A bare word offers the field prefixes themselves, teaching
            // the syntax in place. Two chars before the menu pops, so it
            // stays out of the way of ordinary title typing; a colon
            // here means an unknown field, which stays quiet.
            let typed = strip(raw);
            if typed.len() < 2 || raw.contains(':') {
                return none();
            }
            let span =
                lsp_types::Range::new(text.offset_to_position(start), text.offset_to_position(end));
            QUERY_FIELDS
                .iter()
                .filter(|(name, _)| name.starts_with(&typed))
                .map(|(name, _)| {
                    let term = format!("{name}:");
                    CompletionItem {
                        label: term.clone(),
                        filter_text: Some(name[..typed.len()].to_string()),
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range: span,
                            new_text: term,
                        })),
                        ..Default::default()
                    }
                })
                .collect()
        };
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(
        &self,
        _offset: usize,
        _new_text: &str,
        _cx: &mut Context<InputState>,
    ) -> bool {
        // Requery every keystroke; completions() itself goes quiet
        // outside a field term.
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The highlight length always sits on a char boundary of the label
    /// and never runs past it - the menu's fallback did both and
    /// panicked gpui on labels shorter than the typed token.
    #[test]
    fn matched_prefix_stays_inside_the_label() {
        // Plain prefix match, case-folded.
        assert_eq!(matched_prefix_len("Daft Punk", "daf"), 3);
        // A label shorter than the typed token: nothing to highlight.
        assert_eq!(matched_prefix_len("Exept", "chiyoko"), 0);
        // A contains-match is not a prefix: no highlight.
        assert_eq!(matched_prefix_len("Daft Punk", "punk"), 0);
        // Multi-byte labels highlight whole chars.
        assert_eq!(matched_prefix_len("Ólafur Arnalds", "ól"), 3);
        assert_eq!(matched_prefix_len("Ólafur Arnalds", "x"), 0);
        // Nothing typed, nothing highlighted.
        assert_eq!(matched_prefix_len("Daft Punk", ""), 0);
    }

    /// Tokens resolve under the cursor and classify into field terms
    /// and free words; gaps and unknown prefixes stay quiet.
    #[test]
    fn tokens_resolve_and_classify_under_the_cursor() {
        let text = "stronger artist:daf";
        // Cursor in the first word takes that token; it is a free term.
        assert_eq!(token_at(text, 4), Some((0, 8)));
        assert_eq!(field_term("stronger"), None);
        // Cursor at the end takes the artist term; the value starts
        // after the colon.
        assert_eq!(token_at(text, 19), Some((9, 19)));
        assert_eq!(field_term("artist:daf"), Some((QueryField::Artist, 7)));
        // An empty value right after the colon still counts.
        assert_eq!(field_term("artist:"), Some((QueryField::Artist, 7)));
        // A quoted value keeps its spaces inside one token.
        assert_eq!(token_at("artist:\"daft pu", 15), Some((0, 15)));
        // An unknown prefix is not a field term.
        assert_eq!(field_term("ac:dc"), None);
        // A cursor in trailing whitespace has no token.
        assert_eq!(token_at("artist:x ", 9), None);
    }
}
