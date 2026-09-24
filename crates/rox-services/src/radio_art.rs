//! A guessed cover for the song a station is playing, searched on the
//! announced artist and title per turnover. Never written anywhere: it's a
//! guess from two strings shown behind a blur, unlike the cover picker's
//! confirmed match. A candidate has to score against the title to show;
//! otherwise the station's own picture stands. [`StationArt`] holds that
//! precedence and the staleness rule, apart from the entity so both test
//! without a clock.

use rox_net::providers::{self, TrackQuery};
use rox_playback::IcyTitle;

/// The same bar the Discord presence uses.
const ART_MATCH_BAR: f32 = 0.5;

/// The bytes are blurred to a thumbnail on arrival, so anything bigger is
/// wasted bandwidth.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// None when either half is missing: half a name can't tell a right cover
/// from a wrong one.
pub fn query(title: &IcyTitle) -> Option<TrackQuery> {
    let artist = title.artist.trim();
    let name = title.title.trim();
    if artist.is_empty() || name.is_empty() {
        return None;
    }

    Some(TrackQuery {
        artist: artist.to_string(),
        title: name.to_string(),
        album: String::new(),
        duration_secs: None,
    })
}

/// Blocking, bounded by the provider agent's ten second timeout.
pub fn lookup(query: &TrackQuery) -> Option<Vec<u8>> {
    if !providers::art_online() {
        return None;
    }

    let candidates = match providers::search_art(query) {
        Ok(candidates) => candidates,
        Err(e) => {
            log::debug!("radio art: search failed: {e}");
            return None;
        }
    };

    // Provider order is by pixel size, which says nothing about the match.
    // Ties fall to the largest.
    let chosen = candidates
        .iter()
        .map(|candidate| (candidate, providers::art_confidence(query, candidate)))
        .filter(|(_, score)| *score >= ART_MATCH_BAR)
        .min_by(|(_, a), (_, b)| b.total_cmp(a))
        .map(|(candidate, _)| candidate)?;

    let bytes = match providers::fetch_image(&chosen.full_url) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::debug!("radio art: fetch failed: {e}");
            return None;
        }
    };

    if bytes.len() > MAX_BYTES {
        log::debug!("radio art: {} bytes is past the cap", bytes.len());
        return None;
    }

    Some(bytes)
}

/// The song on air's cover wins when found; the row's own picture falls in
/// behind.
#[derive(Default)]
pub struct StationArt {
    title: Option<Vec<u8>>,
    /// A station's favicon, a Subsonic song's stored cover.
    station: Option<Vec<u8>>,
    /// Stamps the lookups: a slow reply can land after several turnovers,
    /// and the wrong song's cover is worse than none.
    generation: u64,
}

impl StationArt {
    pub fn arm(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// False when the stamp is old: the song is no longer on air.
    pub fn land(&mut self, generation: u64, bytes: Option<Vec<u8>>) -> bool {
        if generation != self.generation {
            return false;
        }

        self.title = bytes;
        true
    }

    pub fn set_station(&mut self, bytes: Option<Vec<u8>>) {
        self.station = bytes;
    }

    pub fn current(&self) -> Option<&[u8]> {
        self.title.as_deref().or(self.station.as_deref())
    }

    /// Bumps the stamp, so a lookup for the last station can't land here.
    pub fn clear(&mut self) {
        self.generation += 1;
        self.title = None;
        self.station = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn title(artist: &str, name: &str) -> IcyTitle {
        IcyTitle {
            artist: artist.to_string(),
            title: name.to_string(),
        }
    }

    #[test]
    fn a_full_title_searches_on_artist_and_song() {
        let query = query(&title("Miles Davis", "So What")).expect("a search");

        assert_eq!(query.artist, "Miles Davis");
        assert_eq!(query.title, "So What");
        assert!(query.album.is_empty(), "a station names no album");
        assert_eq!(query.duration_secs, None, "and a stream has no length");
    }

    #[test]
    fn half_a_title_does_not_search() {
        assert!(query(&title("", "So What")).is_none());
        assert!(query(&title("Miles Davis", "")).is_none());
        assert!(query(&title("  ", "  ")).is_none());
    }

    #[test]
    fn the_songs_cover_outranks_the_stations() {
        let mut art = StationArt::default();
        assert_eq!(art.current(), None, "a station with neither shows neither");

        art.set_station(Some(b"favicon".to_vec()));
        assert_eq!(art.current(), Some(b"favicon".as_slice()));

        let generation = art.arm();
        assert!(art.land(generation, Some(b"cover".to_vec())));
        assert_eq!(art.current(), Some(b"cover".as_slice()));

        let generation = art.arm();
        assert!(art.land(generation, None));
        assert_eq!(art.current(), Some(b"favicon".as_slice()));
    }

    #[test]
    fn a_reply_for_a_song_that_went_past_is_dropped() {
        let mut art = StationArt::default();
        art.set_station(Some(b"favicon".to_vec()));

        let slow = art.arm();
        let current = art.arm();

        assert!(art.land(current, Some(b"cover".to_vec())));
        assert!(!art.land(slow, Some(b"stale".to_vec())), "the old song");
        assert_eq!(art.current(), Some(b"cover".as_slice()));
    }

    #[test]
    fn leaving_the_station_drops_everything() {
        let mut art = StationArt::default();
        art.set_station(Some(b"favicon".to_vec()));
        let pending = art.arm();

        art.clear();
        assert_eq!(art.current(), None);
        assert!(!art.land(pending, Some(b"cover".to_vec())));
        assert_eq!(art.current(), None);
    }
}
