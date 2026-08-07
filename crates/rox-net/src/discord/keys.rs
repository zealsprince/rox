//! rox's Discord application identity: the application id rich
//! presence connects to the local Discord socket with, registered once
//! at discord.com/developers. It comes from the build environment,
//! `DISCORD_APPLICATION_ID`, the same way Last.fm's pair does
//! ([`crate::lastfm::keys`]). A build without it ships no identity and
//! presence stays off; a fork wanting its own presence card registers
//! an application and exports the var. Unlike an api secret this is
//! public by design - Discord shows it on the application page.

pub const APPLICATION_ID: &str = match option_env!("DISCORD_APPLICATION_ID") {
    Some(id) => id,
    None => "",
};
