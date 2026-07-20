//! M3U8 read and write, the interop surface for playlists (ADR 16). Export
//! writes extended M3U (`#EXTM3U` with an `#EXTINF` per track) so a rox
//! playlist opens in VLC, foobar, MPD, and the like. Import reads the same
//! shape back, or any bare list of paths, and hands the caller the entries in
//! order; resolving those paths to catalog tracks is the library's job.
//!
//! The store is still the source of truth, files are a snapshot you generate
//! and re-read, never where playlists live.

use rox_library::playlists::ExportTrack;

/// Serialize playable rows to an extended M3U8 document. Each track gets an
/// `#EXTINF:<secs>,<artist> - <title>` line then its absolute path; a missing
/// artist collapses to just the title, an unknown duration writes `-1` the way
/// the format expects.
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

/// Pull the path entries out of an M3U document in order. Comment and
/// directive lines (`#EXTM3U`, `#EXTINF`, ...) and blanks fall away, so what
/// is left is the file references, whether the input was extended M3U or a
/// bare path list.
pub fn parse(text: &str) -> Vec<String> {
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
