//! Tag value suggestions: a completion provider over the library's distinct
//! values for one field. Accepting an item replaces the whole input (or a
//! genre list's last segment) through its text edit, so multi-word values go
//! in whole even from a mid-word match.

use std::rc::Rc;

use gpui::{App, Context, Entity, Task, Window};
use gpui_component::input::{CompletionProvider, InputState, Rope, RopeExt as _};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionResponse, CompletionTextEdit, TextEdit,
};

use rox_library::projection::{Projection, QUERY_FIELDS, QueryField, SymTable};
use rox_library::writer::Field;
use rox_services::catalog::Library;

/// Most suggestions the menu shows at once.
const CAP: usize = 20;

/// The label bytes whose case-fold matches `typed` as a prefix, for each
/// item's filter_text. The menu's own fallback lands mid-char or past the end
/// on short and non-ascii labels and trips gpui's char boundary assert.
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
    0
}

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

fn ranked_values<'a>(
    values: Vec<&'a String>,
    lower: impl Fn(&str) -> String,
    typed: &str,
) -> Vec<&'a String> {
    let mut prefixed = Vec::new();
    let mut contained = Vec::new();
    for value in values {
        let folded = lower(value);
        if folded.starts_with(typed) {
            prefixed.push(value);
        } else if folded.contains(typed) {
            contained.push(value);
        }
    }
    prefixed.extend(contained);
    prefixed.truncate(CAP);
    prefixed
}

/// Each item rewrites the whole value span, quoted when it has spaces so the
/// tokenizer keeps it in one piece.
fn value_items(values: Vec<&String>, typed: &str, span: lsp_types::Range) -> Vec<CompletionItem> {
    values
        .into_iter()
        .map(|value| {
            let quoted = if value.chars().any(char::is_whitespace) {
                format!("\"{value}\"")
            } else {
                value.clone()
            };
            CompletionItem {
                label: value.clone(),
                filter_text: Some(value[..matched_prefix_len(value, typed)].to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: span,
                    new_text: quoted,
                })),
                ..Default::default()
            }
        })
        .collect()
}

/// The year column has no symbol table, so this ranks a plain year list.
/// `years` comes newest first.
fn ranked_years(years: &[u16], typed: &str) -> Vec<String> {
    let mut prefixed = Vec::new();
    let mut contained = Vec::new();
    for &year in years {
        let value = year.to_string();
        if value.starts_with(typed) {
            prefixed.push(value);
            if prefixed.len() >= CAP {
                break;
            }
        } else if contained.len() < CAP && value.contains(typed) {
            contained.push(value);
        }
    }
    prefixed.extend(contained);
    prefixed.truncate(CAP);
    prefixed
}

pub fn provider(
    library: &Entity<Library>,
    field: &Field,
    cx: &App,
) -> Option<Rc<dyn CompletionProvider>> {
    if !matches!(
        field,
        Field::Artist | Field::AlbumArtist | Field::Album | Field::Genre
    ) {
        return None;
    }
    library.read(cx).projection()?;
    Some(Rc::new(FieldSuggestions {
        library: library.clone(),
        field: field.clone(),
    }))
}

/// Holds the catalog, never the projection: a kept `Arc<Projection>` stops a
/// sync patching the library in place, and an editor can stay open for hours.
struct FieldSuggestions {
    library: Entity<Library>,
    field: Field,
}

impl FieldSuggestions {
    fn table<'a>(&self, projection: &'a Projection) -> &'a SymTable {
        match self.field {
            Field::Artist => &projection.artists,
            Field::AlbumArtist => &projection.album_artists,
            Field::Album => &projection.albums,
            // Split terms: offer "Shoegaze", never a whole "Rock; Shoegaze" list.
            _ => projection.genre_terms(),
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
        cx: &mut Context<InputState>,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let catalog = self.library.read(cx);
        let Some(projection) = catalog.projection() else {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        let full = text.to_string();
        // A genre input holds a "; " list, so complete only the segment after
        // the last separator.
        let seg_start = if self.field == Field::Genre {
            full.rfind(';').map_or(0, |i| {
                let seg = &full[i + 1..];
                i + 1 + (seg.len() - seg.trim_start().len())
            })
        } else {
            0
        };
        let typed = full[seg_start..].to_lowercase();
        // An emptied input closes the menu; an empty segment after a separator
        // lists the values from the top.
        if typed.is_empty() && seg_start == 0 {
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        }
        let span = lsp_types::Range::new(
            text.offset_to_position(seg_start),
            text.offset_to_position(text.len()),
        );
        let items = ranked(self.table(projection), &typed)
            .into_iter()
            .map(|value| CompletionItem {
                label: value.clone(),
                filter_text: Some(value[..matched_prefix_len(value, &typed)].to_string()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: span,
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
        // Every keystroke requeries, deletions too, so an emptied field closes
        // the menu. Programmatic fills take the silent path and never hit this.
        true
    }
}

/// Values for the `field:` term under the cursor, and the field prefixes for
/// a bare word that starts one. Plain title searches get no menu.
pub fn query_provider(library: &Entity<Library>, cx: &App) -> Option<Rc<dyn CompletionProvider>> {
    let years = library.read(cx).projection()?.distinct_years();
    Some(Rc::new(QuerySuggestions {
        library: library.clone(),
        // Snapshot once per attach. Callers reattach on a library change.
        years,
    }))
}

struct QuerySuggestions {
    /// Read per keystroke. Holding the projection would pin it and cost every
    /// sync its incremental patch.
    library: Entity<Library>,
    years: Vec<u16>,
}

/// Tokens split on whitespace outside double quotes, same as the projection's
/// parser.
fn token_at(text: &str, offset: usize) -> Option<(usize, usize)> {
    let mut start = None;
    let mut in_quotes = false;
    for (i, c) in text.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if let Some(s) = start.take()
                    && (s..=i).contains(&offset)
                {
                    return Some((s, i));
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

/// The field a token pins and where its value starts. A negating hyphen pins
/// the same field, so `-artist:daf` completes like `artist:daf`.
fn field_term(raw: &str) -> Option<(QueryField, usize)> {
    let colon = raw.find(':')?;
    let name = &raw[..colon];
    if name.contains('"') {
        return None;
    }
    let name = name.to_lowercase();
    let bare = name.strip_prefix('-').unwrap_or(&name);
    let (_, field) = QUERY_FIELDS.iter().find(|(n, _)| *n == bare)?;
    Some((*field, colon + 1))
}

/// The `field:` pins a bare word completes to, plus the bare `-field` absence
/// form when the word is negated. None under two chars, which keeps the menu
/// off ordinary title typing.
fn field_completions(typed: &str) -> Option<Vec<String>> {
    // The floor counts after the hyphen, or "-a" would pop the menu early.
    let (hyphen, name) = match typed.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", typed),
    };
    if name.len() < 2 {
        return None;
    }
    Some(
        QUERY_FIELDS
            .iter()
            .filter(|(field, _)| field.starts_with(name))
            .flat_map(|(field, kind)| {
                let absence = (!hyphen.is_empty() && kind.absence()).then(|| format!("-{field}"));
                [Some(format!("{hyphen}{field}:")), absence]
            })
            .flatten()
            .collect(),
    )
}

/// The handful of comparisons worth one click, not every legal form.
fn numeric_hints(field: QueryField) -> &'static [&'static str] {
    match field {
        QueryField::Rating => &[">=4", ">=3", "5", "0"],
        QueryField::Plays => &["0", ">0", ">10"],
        QueryField::Added => &["<7d", "<30d", "<90d", "<365d"],
        _ => &[],
    }
}

impl CompletionProvider for QuerySuggestions {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _trigger: CompletionContext,
        _window: &mut Window,
        cx: &mut Context<InputState>,
    ) -> Task<anyhow::Result<CompletionResponse>> {
        let none = || Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        let catalog = self.library.read(cx);
        let Some(projection) = catalog.projection() else {
            return none();
        };
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
            let typed = strip(&raw[value..]);
            let span = lsp_types::Range::new(
                text.offset_to_position(start + value),
                text.offset_to_position(end),
            );
            let table = match field {
                QueryField::Artist => &projection.artists,
                QueryField::AlbumArtist => &projection.album_artists,
                QueryField::Album => &projection.albums,
                QueryField::Genre => projection.genre_terms(),
                QueryField::Folder => &projection.folders,
                QueryField::Codec => &projection.codecs,
                // By display name, since a server's stored string is a digest.
                // Only browsable sources.
                QueryField::Source => {
                    let names: Vec<String> = projection
                        .browse_sources()
                        .map(rox_library::cue::source_label)
                        .collect();
                    let names: Vec<&String> = names.iter().collect();

                    return Task::ready(Ok(CompletionResponse::Array(value_items(
                        ranked_values(names, |name| name.to_lowercase(), &typed),
                        &typed,
                        span,
                    ))));
                }
                // No symbol table, and years never need quoting.
                QueryField::Year => {
                    return Task::ready(Ok(CompletionResponse::Array(
                        ranked_years(&self.years, &typed)
                            .into_iter()
                            .map(|value| CompletionItem {
                                label: value.clone(),
                                filter_text: Some(
                                    value[..matched_prefix_len(&value, &typed)].to_string(),
                                ),
                                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                                    range: span,
                                    new_text: value,
                                })),
                                ..Default::default()
                            })
                            .collect(),
                    )));
                }
                // No table either: offer the comparisons people write, so
                // `rating:` opens with ">=4" rather than a dead menu.
                QueryField::Rating | QueryField::Plays | QueryField::Added => {
                    return Task::ready(Ok(CompletionResponse::Array(
                        numeric_hints(field)
                            .iter()
                            .filter(|hint| hint.starts_with(&typed))
                            .map(|hint| CompletionItem {
                                label: hint.to_string(),
                                filter_text: Some(
                                    hint[..matched_prefix_len(hint, &typed)].to_string(),
                                ),
                                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                                    range: span,
                                    new_text: hint.to_string(),
                                })),
                                ..Default::default()
                            })
                            .collect(),
                    )));
                }
                QueryField::Title => return none(),
            };
            value_items(ranked(table, &typed), &typed, span)
        } else {
            // A bare word offers the field terms. A colon here means an unknown
            // field.
            let typed = strip(raw);
            if raw.contains(':') {
                return none();
            }
            let Some(terms) = field_completions(&typed) else {
                return none();
            };
            let span =
                lsp_types::Range::new(text.offset_to_position(start), text.offset_to_position(end));
            terms
                .into_iter()
                .map(|term| CompletionItem {
                    // The typed text is a prefix of every label here.
                    filter_text: Some(term[..typed.len()].to_string()),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range: span,
                        new_text: term.clone(),
                    })),
                    label: term,
                    ..Default::default()
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The menu's own fallback panics gpui on labels shorter than the typed
    /// token.
    #[test]
    fn matched_prefix_stays_inside_the_label() {
        assert_eq!(matched_prefix_len("Daft Punk", "daf"), 3);
        // A label shorter than the typed token: nothing to highlight.
        assert_eq!(matched_prefix_len("Exept", "chiyoko"), 0);
        assert_eq!(matched_prefix_len("Daft Punk", "punk"), 0);
        assert_eq!(matched_prefix_len("Ólafur Arnalds", "ól"), 3);
        assert_eq!(matched_prefix_len("Ólafur Arnalds", "x"), 0);
        assert_eq!(matched_prefix_len("Daft Punk", ""), 0);
    }

    #[test]
    fn years_rank_prefix_first() {
        let years = vec![2021u16, 2019, 2010, 1999, 1990];
        assert_eq!(
            ranked_years(&years, ""),
            vec!["2021", "2019", "2010", "1999", "1990"]
        );
        assert_eq!(ranked_years(&years, "20"), vec!["2021", "2019", "2010"]);
        // 2019 contains "19" but doesn't start with it.
        assert_eq!(ranked_years(&years, "19"), vec!["1999", "1990", "2019"]);
    }

    #[test]
    fn tokens_resolve_and_classify_under_the_cursor() {
        let text = "stronger artist:daf";
        assert_eq!(token_at(text, 4), Some((0, 8)));
        assert_eq!(field_term("stronger"), None);
        assert_eq!(token_at(text, 19), Some((9, 19)));
        assert_eq!(field_term("artist:daf"), Some((QueryField::Artist, 7)));
        assert_eq!(field_term("artist:"), Some((QueryField::Artist, 7)));
        assert_eq!(token_at("artist:\"daft pu", 15), Some((0, 15)));
        assert_eq!(field_term("ac:dc"), None);
        assert_eq!(token_at("artist:x ", 9), None);
        // The hyphen doesn't turn an unknown prefix into a field.
        assert_eq!(field_term("-artist:daf"), Some((QueryField::Artist, 8)));
        assert_eq!(field_term("-rating:>="), Some((QueryField::Rating, 8)));
        assert_eq!(field_term("-ac:dc"), None);
    }

    #[test]
    fn words_complete_to_field_terms() {
        assert_eq!(field_completions("art").unwrap(), ["artist:"]);
        assert_eq!(field_completions("-art").unwrap(), ["-artist:", "-artist"]);
        // Folder has no absent value, so the negation offers the pin alone.
        assert_eq!(field_completions("-fol").unwrap(), ["-folder:"]);
        assert_eq!(field_completions("al").unwrap(), ["albumartist:", "album:"]);
        // Two chars before the menu pops, counted after the hyphen.
        assert!(field_completions("a").is_none());
        assert!(field_completions("-a").is_none());
        assert!(field_completions("-").is_none());
        assert!(field_completions("-zzz").unwrap().is_empty());
    }
}
