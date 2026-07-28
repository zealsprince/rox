//! Which service identities a build bakes in, and which source each one comes
//! from. Its own file so the build script and `cargo test` both compile it:
//! build.rs can't host tests cargo would run, and the precedence rule below is
//! the part worth pinning down.

use std::path::Path;

/// The identities [`option_env!`] reads, the same set the release workflow
/// passes as repository secrets and `.env.template` documents.
pub const IDENTITY_KEYS: [&str; 3] =
    ["LASTFM_API_KEY", "LASTFM_API_SECRET", "DISCORD_APPLICATION_ID"];

/// The identities the crate should compile with, read out of `env_file`.
/// `exported` answers what the surrounding environment already carries, and
/// whatever it answers wins: that's Node's dotenv rule, and it's what keeps a
/// stray local `.env` from shadowing the secrets CI passes in. A missing or
/// unreadable file is the ordinary case, not an error.
pub fn resolve(
    env_file: &Path,
    exported: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let Ok(vars) = dotenvy::from_path_iter(env_file) else {
        return Vec::new();
    };
    vars.flatten()
        .filter(|(key, _)| IDENTITY_KEYS.contains(&key.as_str()))
        // CI sets every key to its secret's value or to empty when that secret
        // isn't configured, so an empty export counts as no export at all.
        .filter(|(key, _)| exported(key).is_none_or(|value| value.is_empty()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Writes an `.env` under the integration test's scratch directory. Named
    /// per test so the cases don't tread on each other.
    fn env_file(name: &str, body: &str) -> PathBuf {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.env"));
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    /// No exports at all, the plain local build.
    fn nothing_exported(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn reads_the_identities_out_of_the_file() {
        let path = env_file(
            "plain",
            "# a comment\nLASTFM_API_KEY=abc123\nLASTFM_API_SECRET=\"quoted secret\"\n",
        );
        let resolved = resolve(&path, nothing_exported);
        assert_eq!(
            resolved,
            vec![
                ("LASTFM_API_KEY".into(), "abc123".into()),
                ("LASTFM_API_SECRET".into(), "quoted secret".into()),
            ]
        );
    }

    #[test]
    fn keys_the_build_doesnt_bake_in_are_ignored() {
        let path = env_file("strangers", "LASTFM_API_KEY=abc123\nHOME=/somewhere/else\n");
        let resolved = resolve(&path, nothing_exported);
        assert_eq!(resolved, vec![("LASTFM_API_KEY".into(), "abc123".into())]);
    }

    #[test]
    fn an_exported_value_beats_the_file() {
        let path = env_file(
            "shadowed",
            "LASTFM_API_KEY=fromfile\nDISCORD_APPLICATION_ID=123\n",
        );
        let env = HashMap::from([("LASTFM_API_KEY", "fromenv")]);
        let resolved = resolve(&path, |key| env.get(key).map(|v| v.to_string()));
        // The shadowed key drops out entirely: build.rs emits nothing for it,
        // so cargo passes the exported value through untouched.
        assert_eq!(resolved, vec![("DISCORD_APPLICATION_ID".into(), "123".into())]);
    }

    /// The unconfigured-secret case: the workflow's `env:` block still defines
    /// the name, so the build script sees it set to empty.
    #[test]
    fn an_empty_export_counts_as_unset() {
        let path = env_file("empty_export", "LASTFM_API_KEY=fromfile\n");
        let env = HashMap::from([("LASTFM_API_KEY", "")]);
        let resolved = resolve(&path, |key| env.get(key).map(|v| v.to_string()));
        assert_eq!(resolved, vec![("LASTFM_API_KEY".into(), "fromfile".into())]);
    }

    #[test]
    fn a_missing_file_resolves_to_nothing() {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("does-not-exist.env");
        assert!(resolve(&path, nothing_exported).is_empty());
    }
}
