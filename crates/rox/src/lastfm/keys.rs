//! rox's Last.fm api identity: the key pair the scrobbler signs its
//! calls with, registered once at Last.fm/api/account/create. The pair
//! is baked in from the build environment, `LASTFM_API_KEY` and
//! `LASTFM_API_SECRET`, which is how the release workflow hands the
//! repository secrets to cargo. A build without them ships no identity,
//! and the settings page asks the user for their own pair instead; a
//! fork wanting one-click connect registers its own account and exports
//! the two vars. The secret riding along in the binary is the usual
//! open-source scrobbler trade-off: it identifies the app, not any
//! user, and accounts still authorize per session in the browser.

pub const API_KEY: &str = match option_env!("LASTFM_API_KEY") {
    Some(key) => key,
    None => "",
};

pub const API_SECRET: &str = match option_env!("LASTFM_API_SECRET") {
    Some(secret) => secret,
    None => "",
};
