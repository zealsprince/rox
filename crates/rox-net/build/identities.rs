//! Which service identities a build bakes in, and from where. Its own file
//! so build.rs and `cargo test` both compile it.

use std::path::Path;

/// Keep in sync with the release workflow's secrets and `.env.template`.
pub const IDENTITY_KEYS: [&str; 4] = [
    "LASTFM_API_KEY",
    "LASTFM_API_SECRET",
    "DISCORD_APPLICATION_ID",
    "ACOUSTID_CLIENT_KEY",
];

/// The identities to compile with out of `env_file`. Whatever `exported`
/// answers wins (dotenv's rule), so a stray local `.env` never shadows the
/// secrets CI passes in.
pub fn resolve(
    env_file: &Path,
    exported: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let Ok(vars) = dotenvy::from_path_iter(env_file) else {
        return Vec::new();
    };
    vars.flatten()
        .filter(|(key, _)| IDENTITY_KEYS.contains(&key.as_str()))
        // CI exports an unconfigured secret as empty, which counts as unset.
        .filter(|(key, _)| exported(key).is_none_or(|value| value.is_empty()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::path::PathBuf;

    fn env_file(name: &str, body: &str) -> PathBuf {
        let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.env"));
        std::fs::write(&path, body).expect("write fixture");
        path
    }

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
        assert_eq!(
            resolved,
            vec![("DISCORD_APPLICATION_ID".into(), "123".into())]
        );
    }

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
