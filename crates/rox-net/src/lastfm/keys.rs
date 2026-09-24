//! rox's Last.fm api key pair, baked in from `LASTFM_API_KEY` and
//! `LASTFM_API_SECRET` (the release workflow's secrets). A build without
//! them asks the user for their own pair. Shipping the secret is the usual
//! open-source scrobbler trade-off: it identifies the app, not a user.

pub const API_KEY: &str = match option_env!("LASTFM_API_KEY") {
    Some(key) => key,
    None => "",
};

pub const API_SECRET: &str = match option_env!("LASTFM_API_SECRET") {
    Some(secret) => secret,
    None => "",
};
