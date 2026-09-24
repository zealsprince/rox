//! Find and replace over a batch, PowerRename's idea pointed at tags. The
//! find is literal unless regex is asked for, where the replacement expands
//! `$1` and `${name}`. Every match rewrites, a result trims its edges, and
//! an empty value never changes.

use regex::{NoExpand, Regex, RegexBuilder};

#[derive(Debug)]
pub struct Rule {
    re: Regex,
    replacement: String,
    /// Literal mode inserts the replacement as typed, `$` and all.
    expand: bool,
}

/// None for an empty find, the panel's resting state.
pub fn compile(
    find: &str,
    replacement: &str,
    regex: bool,
    ignore_case: bool,
) -> Result<Option<Rule>, String> {
    if find.is_empty() {
        return Ok(None);
    }
    let pattern = if regex {
        find.to_owned()
    } else {
        regex::escape(find)
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|e| describe(&e))?;
    Ok(Some(Rule {
        re,
        replacement: replacement.to_owned(),
        expand: regex,
    }))
}

/// The crate's display spans several lines; the last one says what's wrong.
fn describe(e: &regex::Error) -> String {
    let text = e.to_string();
    text.lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("error: "))
        .unwrap_or_else(|| text.trim())
        .to_owned()
}

impl Rule {
    pub fn apply(&self, value: &str) -> Option<String> {
        if value.is_empty() {
            return None;
        }
        let out = if self.expand {
            self.re.replace_all(value, self.replacement.as_str())
        } else {
            self.re.replace_all(value, NoExpand(&self.replacement))
        };
        let out = out.trim();
        (out != value).then(|| out.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::compile;

    fn run(find: &str, with: &str, regex: bool, ignore_case: bool, value: &str) -> Option<String> {
        compile(find, with, regex, ignore_case)
            .expect("a valid rule")
            .expect("a non-empty find")
            .apply(value)
    }

    #[test]
    fn a_literal_find_takes_its_parens_as_text() {
        assert_eq!(
            run("(Remastered)", "", false, false, "Song (Remastered)"),
            Some("Song".to_owned())
        );
    }

    #[test]
    fn a_literal_replacement_keeps_its_dollars() {
        assert_eq!(run("a", "$1", false, false, "a"), Some("$1".to_owned()));
    }

    #[test]
    fn a_regex_expands_its_groups() {
        assert_eq!(
            run(r"(.*) - (.*)", "$2 - $1", true, false, "Artist - Title"),
            Some("Title - Artist".to_owned())
        );
        assert_eq!(
            run(
                r"\s*\((Remastered|Explicit)\)",
                "",
                true,
                false,
                "Song (Explicit)"
            ),
            Some("Song".to_owned())
        );
    }

    #[test]
    fn every_match_rewrites() {
        assert_eq!(
            run("a", "b", false, false, "a a a"),
            Some("b b b".to_owned())
        );
    }

    #[test]
    fn case_is_ignored_only_when_asked() {
        assert_eq!(
            run("(remastered)", "", false, false, "Song (REMASTERED)"),
            None
        );
        assert_eq!(
            run("(remastered)", "", false, true, "Song (REMASTERED)"),
            Some("Song".to_owned())
        );
    }

    #[test]
    fn a_result_trims_its_edges() {
        assert_eq!(
            run("Remastered", "", false, false, "  Song Remastered "),
            Some("Song".to_owned())
        );
    }

    #[test]
    fn an_untouched_or_empty_value_changes_nothing() {
        assert_eq!(run("x", "y", false, false, "Song"), None);
        assert_eq!(run("x", "y", false, false, ""), None);
        assert_eq!(run(r"^", "y", true, false, ""), None);
    }

    #[test]
    fn a_whole_match_empties_the_value() {
        assert_eq!(run("Song", "", false, false, "Song"), Some(String::new()));
    }

    #[test]
    fn an_empty_find_is_nothing_to_do() {
        assert!(compile("", "y", false, false).unwrap().is_none());
        assert!(compile("", "y", true, false).unwrap().is_none());
    }

    #[test]
    fn a_bad_regex_says_so_in_one_line() {
        let error = compile("(oops", "", true, false).unwrap_err();
        assert!(!error.contains('\n'), "{error:?}");
        assert!(!error.is_empty());
        assert!(compile("(oops", "", false, false).is_ok());
    }
}
