//! PLS read and write for playlist interop (ADR 16). Import only reads the
//! `File` keys; the rest is display metadata the catalog already holds.

use crate::playlists::ExportTrack;

/// A PLS document with the same `Title` and `-1` duration rules as
/// [`crate::m3u`].
pub fn to_pls(rows: &[ExportTrack]) -> String {
    let mut out = String::from("[playlist]\n");
    for (i, row) in rows.iter().enumerate() {
        let n = i + 1;
        let secs = if row.duration_secs > 0 {
            row.duration_secs
        } else {
            -1
        };
        let display = if row.artist.is_empty() {
            row.title.clone()
        } else {
            format!("{} - {}", row.artist, row.title)
        };
        out.push_str(&format!("File{n}={}\n", row.path));
        out.push_str(&format!("Title{n}={display}\n"));
        out.push_str(&format!("Length{n}={secs}\n"));
    }
    out.push_str(&format!("NumberOfEntries={}\n", rows.len()));
    out.push_str("Version=2\n");
    out
}

/// The path entries ordered by entry number rather than line, since that's
/// what the format says to trust. Keys match case-insensitively.
pub fn parse(text: &str) -> Vec<String> {
    // A leading BOM would cling to the first key.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut entries: Vec<(u32, String)> = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let key = key.trim();
        let Some(index) = key
            .get(..4)
            .filter(|head| head.eq_ignore_ascii_case("file"))
            .and_then(|_| key[4..].trim().parse::<u32>().ok())
        else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        entries.push((index, value.to_owned()));
    }
    entries.sort_by_key(|(index, _)| *index);
    entries.into_iter().map(|(_, value)| value).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, artist: &str, title: &str, secs: i64) -> ExportTrack {
        ExportTrack {
            path: path.into(),
            title: title.into(),
            artist: artist.into(),
            duration_secs: secs,
        }
    }

    #[test]
    fn writes_the_ini_triples_and_the_trailer() {
        let pls = to_pls(&[
            row("/m/one.mp3", "A", "One", 210),
            row("/m/two.mp3", "", "Two", 0),
        ]);
        assert_eq!(
            pls,
            "[playlist]\n\
             File1=/m/one.mp3\nTitle1=A - One\nLength1=210\n\
             File2=/m/two.mp3\nTitle2=Two\nLength2=-1\n\
             NumberOfEntries=2\nVersion=2\n"
        );
    }

    #[test]
    fn round_trips_paths() {
        let rows = [
            row("/m/a.flac", "Artist", "A", 5),
            row("/m/b.flac", "Artist", "B", 6),
        ];
        assert_eq!(parse(&to_pls(&rows)), ["/m/a.flac", "/m/b.flac"]);
    }

    #[test]
    fn entry_numbers_beat_line_order() {
        let text = "[playlist]\n\
                    File2=/m/two.mp3\n\
                    File10=/m/ten.mp3\n\
                    File1=/m/one.mp3\n\
                    NumberOfEntries=3\n";
        assert_eq!(parse(text), ["/m/one.mp3", "/m/two.mp3", "/m/ten.mp3"]);
    }

    #[test]
    fn strips_a_bom_and_takes_lowercase_keys() {
        let text = "\u{feff}[playlist]\nfile1=/m/one.mp3\nFILE2 = /m/two.mp3\n";
        assert_eq!(parse(text), ["/m/one.mp3", "/m/two.mp3"]);
    }

    #[test]
    fn ignores_everything_that_is_not_a_file_key() {
        let text = "[playlist]\n\
                    Title1=Not a path\n\
                    Length1=210\n\
                    File1=/m/one.mp3\n\
                    Fileless=/m/nope.mp3\n\
                    NumberOfEntries=1\n\
                    Version=2\n";
        assert_eq!(parse(text), ["/m/one.mp3"]);
    }
}
