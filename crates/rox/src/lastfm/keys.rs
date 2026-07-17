//! rox's last.fm api identity: the key pair the scrobbler signs its
//! calls with, registered once at last.fm/api/account/create. Empty
//! constants mean the build ships no identity and the settings page
//! asks the user for their own pair instead; a fork wanting one-click
//! connect registers its own account and fills these in. The secret
//! being readable here is the usual open-source scrobbler trade-off:
//! it identifies the app, not any user - accounts still authorize per
//! session in the browser.

pub const API_KEY: &str = "";
pub const API_SECRET: &str = "";
