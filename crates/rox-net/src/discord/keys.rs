//! rox's Discord application id, baked in from `DISCORD_APPLICATION_ID`
//! like the Last.fm pair. A build without it runs with presence off. The id
//! is public: Discord shows it on the application page.

pub const APPLICATION_ID: &str = match option_env!("DISCORD_APPLICATION_ID") {
    Some(id) => id,
    None => "",
};
