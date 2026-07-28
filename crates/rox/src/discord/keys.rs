//! rox's Discord application identity: the client id rich presence
//! connects to the local Discord socket with, registered once at
//! discord.com/developers. It comes from the build environment,
//! `DISCORD_CLIENT_ID`, the same way last.fm's pair does
//! ([`crate::lastfm::keys`]). A build without it ships no identity and
//! presence stays off; a fork wanting its own presence card registers
//! an application and exports the var. Unlike an api secret this is
//! public by design - Discord shows it on the application page.

pub const CLIENT_ID: &str = match option_env!("DISCORD_CLIENT_ID") {
    Some(id) => id,
    None => "",
};
