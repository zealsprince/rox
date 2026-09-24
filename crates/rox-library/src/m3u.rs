//! M3U8 read and write for playlist interop (ADR 16). Export writes extended
//! M3U; import takes that or any bare path list. The store stays the source
//! of truth, files are snapshots.

use crate::playlists::ExportTrack;

/// Extended M3U8: `#EXTINF:<secs>,<artist> - <title>` then the path, `-1` for
/// an unknown duration.
pub fn to_m3u8(rows: &[ExportTrack]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for row in rows {
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
        out.push_str(&format!("#EXTINF:{secs},{display}\n"));
        out.push_str(&row.path);
        out.push('\n');
    }
    out
}

/// The path entries in order, directives and blanks dropped.
pub fn parse(text: &str) -> Vec<String> {
    // A leading BOM would hide `#EXTM3U` from the comment filter.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
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
    fn writes_extinf_and_paths() {
        let m3u = to_m3u8(&[
            row("/m/one.mp3", "A", "One", 210),
            row("/m/two.mp3", "", "Two", 0),
        ]);
        assert_eq!(
            m3u,
            "#EXTM3U\n\
             #EXTINF:210,A - One\n/m/one.mp3\n\
             #EXTINF:-1,Two\n/m/two.mp3\n"
        );
    }

    #[test]
    fn parse_keeps_paths_drops_directives() {
        let text = "#EXTM3U\n\
                    #EXTINF:210,A - One\n  /m/one.mp3  \n\
                    \n\
                    relative/two.mp3\n";
        assert_eq!(parse(text), ["/m/one.mp3", "relative/two.mp3"]);
    }

    #[test]
    fn round_trips_paths() {
        let rows = [
            row("/m/a.flac", "Artist", "A", 5),
            row("/m/b.flac", "Artist", "B", 6),
        ];
        let back = parse(&to_m3u8(&rows));
        assert_eq!(back, ["/m/a.flac", "/m/b.flac"]);
    }
}
