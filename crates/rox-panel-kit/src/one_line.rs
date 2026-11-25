//! Text shaped as one raw line outside an input, such as canvas text. gpui
//! shapes such a line in one call that asserts there's no line break in it,
//! so a newline there is a panic in a debug build. Single-line inputs strip
//! their own, in the vendored widget.

use std::borrow::Cow;

/// `text` without its line breaks.
pub fn one_line(text: &str) -> Cow<'_, str> {
    if text.contains(['\n', '\r']) {
        Cow::Owned(text.replace(['\n', '\r'], ""))
    } else {
        Cow::Borrowed(text)
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
