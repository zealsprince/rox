//! Text a single-line input or a raw shaped line holds. gpui shapes such a
//! line in one call that asserts there's no line break in it, so a newline
//! there is a panic in a debug build. A single-line input refuses one only
//! on paste: an Enter that nothing handles reaches it as typed text, and a
//! stored value goes in as it was written.

use std::borrow::Cow;

use gpui_component::input::InputState;

/// `text` without its line breaks.
pub fn one_line(text: &str) -> Cow<'_, str> {
    if text.contains(['\n', '\r']) {
        Cow::Owned(text.replace(['\n', '\r'], ""))
    } else {
        Cow::Borrowed(text)
    }
}

pub trait OneLine {
    /// Refuses an edit that would leave a line break in the text. Takes the
    /// input's one validator, so it can't sit beside another.
    fn one_line(self) -> Self;
}

impl OneLine for InputState {
    fn one_line(self) -> Self {
        self.validate(|text, _| !text.contains(['\n', '\r']))
    }
}

#[cfg(test)]
mod tests {
    use super::one_line;

    #[test]
    fn line_breaks_go_and_the_rest_stays() {
        assert_eq!(
            one_line("/opt/bin:\n/usr/local/bin\r\n"),
            "/opt/bin:/usr/local/bin"
        );
        assert_eq!(one_line("\n"), "");
        assert!(matches!(
            one_line("plain"),
            std::borrow::Cow::Borrowed("plain")
        ));
    }
}
