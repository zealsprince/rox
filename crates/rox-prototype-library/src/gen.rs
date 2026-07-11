//! Deterministic synthetic catalog. Cardinalities model a real collection:
//! artists own a handful of albums, albums hold 5-16 tracks, so 10M tracks
//! lands around 270k artists and 950k albums. Artist names are unique by
//! construction and album names unique per artist, which makes every path
//! unique without a synthetic suffix.

use crate::TrackRow;

const WORDS: &[&str] = &[
    "amber", "arc", "ash", "atlas", "aurora", "autumn", "birch", "blue", "bone", "border",
    "breath", "bright", "broken", "canyon", "cedar", "chalk", "chrome", "cinder", "circle",
    "city", "cloud", "coast", "cobalt", "cold", "copper", "coral", "crane", "crimson", "crystal",
    "dawn", "delta", "drift", "dusk", "dust", "echo", "ember", "empire", "evening", "fable",
    "fall", "feather", "fern", "field", "fire", "flood", "fog", "forest", "fox", "frost",
    "garden", "ghost", "glass", "gold", "granite", "grove", "harbor", "haze", "heart", "hollow",
    "honey", "horizon", "hour", "hunter", "ice", "iron", "island", "ivory", "jade", "june",
    "lantern", "lark", "light", "lily", "linen", "lion", "low", "lunar", "maple", "marble",
    "meadow", "midnight", "mirror", "mist", "moon", "moss", "mountain", "murmur", "neon",
    "night", "north", "oak", "ocean", "old", "opal", "orbit", "orchid", "owl", "paper", "pearl",
    "pine", "prairie", "quiet", "rain", "raven", "red", "reed", "river", "road", "rose", "ruby",
    "rust", "sable", "salt", "sand", "scarlet", "sea", "shadow", "signal", "silver", "sky",
    "slate", "slow", "smoke", "snow", "solar", "sparrow", "spring", "star", "static", "steel",
    "stone", "storm", "summer", "sun", "swan", "thistle", "thorn", "thunder", "tide", "timber",
    "topaz", "trace", "umber", "valley", "veil", "velvet", "violet", "wave", "west", "whale",
    "willow", "wind", "winter", "wire", "wolf", "wren",
];

const GENRES: &[&str] = &[
    "Rock", "Pop", "Jazz", "Classical", "Electronic", "Hip-Hop", "Folk", "Metal", "Ambient",
    "Blues", "Country", "Reggae", "Punk", "Soul", "Funk", "House", "Techno", "Indie",
    "Soundtrack", "Experimental", "Latin", "R&B", "Gospel", "World",
];

/// SplitMix64, seeded, so every run at a given track count is identical.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

fn title_case(buf: &mut String, word: &str) {
    let mut chars = word.chars();
    if let Some(c) = chars.next() {
        buf.extend(c.to_uppercase());
        buf.push_str(chars.as_str());
    }
}

/// A random name from 1..=max_words dictionary words. Not unique.
fn name(rng: &mut Rng, max_words: u64) -> String {
    let mut out = String::new();
    let words = 1 + rng.below(max_words);
    for i in 0..words {
        if i > 0 {
            out.push(' ');
        }
        title_case(&mut out, WORDS[rng.below(WORDS.len() as u64) as usize]);
    }
    out
}

/// A globally unique name: two words indexed by the counter, a numeric suffix
/// once the combination space wraps. Unique artist names keep every generated
/// path unique without a synthetic filename suffix.
fn unique_name(counter: u64) -> String {
    let w = WORDS.len() as u64;
    let mut out = String::new();
    title_case(&mut out, WORDS[(counter % w) as usize]);
    out.push(' ');
    title_case(&mut out, WORDS[((counter / w) % w) as usize]);
    let wrap = counter / (w * w);
    if wrap > 0 {
        out.push_str(&format!(" {}", wrap + 1));
    }
    out
}

/// Stream `total` tracks into `sink`, artist by artist, album by album.
pub fn generate(seed: u64, total: u64, mut sink: impl FnMut(TrackRow)) {
    let mut rng = Rng::new(seed);
    let mut emitted: u64 = 0;
    let mut artist_no: u64 = 0;

    while emitted < total {
        let artist = unique_name(artist_no);
        artist_no += 1;
        let albums = 1 + rng.below(6);
        for album_no in 0..albums {
            if emitted >= total {
                break;
            }
            let mut album = name(&mut rng, 3);
            if album_no > 0 {
                // Unique per artist is enough for unique paths.
                album.push_str(&format!(" Vol {}", album_no + 1));
            }
            let genre = GENRES[rng.below(GENRES.len() as u64) as usize];
            let year = 1955 + rng.below(70) as u16;
            let tracks = 5 + rng.below(12);
            for t in 0..tracks {
                if emitted >= total {
                    break;
                }
                let title = name(&mut rng, 4);
                let track_no = (t + 1) as u16;
                let duration_ms = (90_000 + rng.below(330_000)) as u32;
                sink(TrackRow {
                    path: format!("/music/{artist}/{album}/{track_no:02} {title}.flac"),
                    title,
                    artist: artist.clone(),
                    album: album.clone(),
                    genre,
                    year,
                    track_no,
                    duration_ms,
                    size: duration_ms as u64 * 110,
                    mtime: 1_600_000_000 + emitted as i64,
                });
                emitted += 1;
            }
        }
    }
}
