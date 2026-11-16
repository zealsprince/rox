//! The catalog's wire side (ADR 36): the index, a package's record and its
//! previews, read from one origin and parsed into plain data. It never
//! writes a file; the package download is [`crate::download`].
//!
//! Everything here is a stranger's bytes. Every read is capped before it's
//! parsed, no redirect is followed, and no URL the catalog serves is ever
//! fetched as given: a `file` or `preview` is a plain relative path that
//! [`Origin::resolve`] puts under the origin, and anything else costs the
//! entry that carried it. A package id is a plain slug before it reaches a
//! URL, a file name or a log line.

use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::download::{HTTPS_ONLY, is_sha256, refuse_redirect, stated_length};
use crate::providers::{USER_AGENT, net_reason};

/// Placeholders from the catalog contract, to tune once a real index exists.
pub const INDEX_MAX: u64 = 4 << 20;
pub const RECORD_MAX: u64 = 256 << 10;
pub const PREVIEW_MAX: u64 = 2 << 20;

/// The most a package's file may state. An entry or a version stating more
/// is dropped before anything could download it.
pub const PACKAGE_MAX: u64 = 256 << 20;

/// What the contract gives an id. Path components longer than this aren't
/// a slug anyone would type.
const ID_MAX: usize = 64;
const PATH_MAX: usize = 255;

/// The catalog's one host, `https://<host>[:port]` with nothing after it.
/// Only a test or debug build takes plain http, and only to 127.0.0.1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin(String);

impl Origin {
    pub fn new(text: &str) -> Result<Origin, String> {
        let authority = text
            .strip_prefix("https://")
            .or_else(|| loopback(text))
            .ok_or_else(|| format!("{text} isn't an https origin"))?;

        // A host and a port, nothing else: no path, query, fragment or
        // credentials can ride in on the origin itself.
        let plain = !authority.is_empty()
            && authority
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b));
        if !plain {
            return Err(format!("{text} isn't a bare origin"));
        }

        Ok(Origin(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The URL for a path under `/v1/`, checked every time so nothing the
    /// index carries is ever fetched as given.
    pub fn resolve(&self, path: &str) -> Result<String, String> {
        if !plain_path(path) {
            return Err("the catalog named a path that isn't a plain relative one".into());
        }

        Ok(format!("{}/v1/{path}", self.0))
    }
}

#[cfg(any(test, debug_assertions))]
fn loopback(text: &str) -> Option<&str> {
    let authority = text.strip_prefix("http://")?;
    (authority == "127.0.0.1" || authority.starts_with("127.0.0.1:")).then_some(authority)
}

#[cfg(not(any(test, debug_assertions)))]
fn loopback(_: &str) -> Option<&str> {
    None
}

/// `[a-z0-9-]`, 1 to 64 characters, starting with a letter or digit. Safe as
/// one URL path segment, one file name and one log token on every platform.
pub fn is_slug(id: &str) -> bool {
    (1..=ID_MAX).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
}

/// Relative, forward slashes, each component `[A-Za-z0-9._-]` and neither
/// `.` nor `..`. That refuses a scheme, a leading slash, `//`, a backslash,
/// a query and percent-encoding in one rule.
pub fn plain_path(path: &str) -> bool {
    (1..=PATH_MAX).contains(&path.len())
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Workspace,
    Plugin,
    Object,
    /// A kind this build doesn't know. The index drops these entries.
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Index {
    pub version: u32,
    pub generated: String,
    pub packages: Vec<Summary>,
    /// Why each refused entry was refused, for the log. Ids are only named
    /// when they're slugs.
    #[serde(skip)]
    pub dropped: Vec<String>,
}

impl Index {
    pub fn get(&self, id: &str) -> Option<&Summary> {
        self.packages.iter().find(|summary| summary.id == id)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Summary {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    pub author: String,
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub preview: Option<String>,
    pub latest: Latest,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Latest {
    /// The author's own string, shown and never compared.
    pub version: String,
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
    pub min_rox: String,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Record {
    pub id: String,
    pub description: String,
    pub homepage: String,
    pub previews: Vec<String>,
    pub versions: Vec<Version>,
    #[serde(skip)]
    pub dropped: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Version {
    pub version: String,
    #[serde(default)]
    pub published: String,
    pub file: String,
    pub bytes: u64,
    pub sha256: String,
    pub min_rox: String,
    #[serde(default)]
    pub notes: String,
}

/// The contract's `/v1/` index is version 1. Anything else is a layout this
/// build can't read.
const INDEX_VERSION: u32 = 1;

#[derive(Deserialize)]
struct RawIndex {
    version: u32,
    #[serde(default)]
    generated: String,
    #[serde(default)]
    packages: Vec<Value>,
}

#[derive(Deserialize)]
struct RawRecord {
    id: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    previews: Vec<String>,
    #[serde(default)]
    versions: Vec<Value>,
}

/// A malformed entry costs that entry only; the rest still load.
pub fn parse_index(text: &str) -> Result<Index, String> {
    let raw: RawIndex =
        serde_json::from_str(text).map_err(|e| format!("the catalog index doesn't read: {e}"))?;
    if raw.version != INDEX_VERSION {
        return Err(format!(
            "the catalog index is version {}, not {INDEX_VERSION}",
            raw.version
        ));
    }

    let mut index = Index {
        version: raw.version,
        generated: raw.generated,
        ..Index::default()
    };

    for (n, entry) in raw.packages.into_iter().enumerate() {
        let summary = match serde_json::from_value::<Summary>(entry) {
            Ok(summary) => summary,
            Err(e) => {
                index.dropped.push(format!("entry {n}: {e}"));
                continue;
            }
        };

        if let Err(reason) = check_summary(&summary) {
            index.dropped.push(format!("entry {n}: {reason}"));
            continue;
        }

        if index.get(&summary.id).is_some() {
            index
                .dropped
                .push(format!("entry {n}: {} is listed twice", summary.id));
            continue;
        }

        index.packages.push(summary);
    }

    Ok(index)
}

fn check_summary(summary: &Summary) -> Result<(), String> {
    if !is_slug(&summary.id) {
        return Err("its id isn't a plain slug".into());
    }

    let id = &summary.id;
    if summary.kind == Kind::Unknown {
        return Err(format!("{id} is a kind this build doesn't know"));
    }

    if summary
        .preview
        .as_deref()
        .is_some_and(|preview| !plain_path(preview))
    {
        return Err(format!("{id}'s preview isn't a plain relative path"));
    }

    if summary.latest.requires.iter().any(|id| !is_slug(id)) {
        return Err(format!("{id} requires something that isn't a plain id"));
    }

    check_file(
        &summary.latest.file,
        summary.latest.bytes,
        &summary.latest.sha256,
    )
    .map_err(|reason| format!("{id}: {reason}"))
}

fn check_file(file: &str, bytes: u64, sha256: &str) -> Result<(), String> {
    if !plain_path(file) {
        return Err("its file isn't a plain relative path".into());
    }

    if bytes == 0 {
        return Err("its file is stated as empty".into());
    }

    if bytes > PACKAGE_MAX {
        return Err(format!(
            "its file is stated as {bytes} bytes, over the {PACKAGE_MAX} byte ceiling"
        ));
    }

    if !is_sha256(sha256) {
        return Err("its sha256 isn't 64 lowercase hex characters".into());
    }

    Ok(())
}

/// The record for `id`. One whose own id differs is refused whole; a bad
/// version or preview costs only itself.
pub fn parse_record(id: &str, text: &str) -> Result<Record, String> {
    let raw: RawRecord =
        serde_json::from_str(text).map_err(|e| format!("the record for {id} doesn't read: {e}"))?;
    if raw.id != id {
        return Err(format!(
            "the record fetched for {id} is for another package"
        ));
    }

    let mut record = Record {
        id: raw.id,
        description: raw.description,
        homepage: raw.homepage,
        ..Record::default()
    };

    for preview in raw.previews {
        if plain_path(&preview) {
            record.previews.push(preview);
        } else {
            record
                .dropped
                .push("a preview that isn't a plain relative path".into());
        }
    }

    for (n, entry) in raw.versions.into_iter().enumerate() {
        let checked = serde_json::from_value::<Version>(entry)
            .map_err(|e| e.to_string())
            .and_then(|version| {
                check_file(&version.file, version.bytes, &version.sha256).map(|()| version)
            });
        match checked {
            Ok(version) => record.versions.push(version),
            Err(reason) => record.dropped.push(format!("version {n}: {reason}")),
        }
    }

    Ok(record)
}

/// Ten seconds for the whole request, like [`crate::providers::agent`], and
/// no redirect followed: the contract says the site never redirects off its
/// origin, so rox follows none.
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .redirects(0)
            .https_only(HTTPS_ONLY)
            .build()
    })
}

fn get(url: &str, cap: u64) -> Result<(String, Vec<u8>), String> {
    let response = agent().get(url).call().map_err(|e| net_reason(&e))?;
    read_capped(response, cap)
}

/// The body and its content type, refused past `cap` without reading more
/// than one byte over it. ureq's own string limit isn't the guard.
fn read_capped(response: ureq::Response, cap: u64) -> Result<(String, Vec<u8>), String> {
    refuse_redirect(&response)?;

    if stated_length(&response).is_some_and(|stated| stated > cap) {
        return Err(format!("the catalog's answer is over its {cap} byte cap"));
    }

    let content_type = response.content_type().to_string();
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > cap {
        return Err(format!("the catalog's answer is over its {cap} byte cap"));
    }

    Ok((content_type, bytes))
}

fn text((_, bytes): (String, Vec<u8>)) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| "the catalog's answer isn't UTF-8".to_string())
}

/// The index exactly as served, for a cache that keeps its bytes.
pub fn fetch_index_text(origin: &Origin) -> Result<String, String> {
    text(get(&origin.resolve("index.json")?, INDEX_MAX)?)
}

pub fn fetch_index(origin: &Origin) -> Result<Index, String> {
    parse_index(&fetch_index_text(origin)?)
}

pub fn fetch_record(origin: &Origin, id: &str) -> Result<Record, String> {
    if !is_slug(id) {
        return Err("that isn't a catalog id".into());
    }

    let url = origin.resolve(&format!("packages/{id}.json"))?;
    parse_record(id, &text(get(&url, RECORD_MAX)?)?)
}

/// A preview's bytes, only when the server calls them an image. Decoding
/// them is the caller's, under its own pixel cap.
pub fn fetch_preview(origin: &Origin, path: &str) -> Result<Vec<u8>, String> {
    image(get(&origin.resolve(path)?, PREVIEW_MAX)?)
}

fn image((content_type, bytes): (String, Vec<u8>)) -> Result<Vec<u8>, String> {
    if !content_type.starts_with("image/") {
        return Err("the catalog served a preview that isn't an image".into());
    }

    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::canned;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn entry(id: &str, kind: &str, file: &str) -> Value {
        serde_json::json!({
            "id": id,
            "kind": kind,
            "name": "A name",
            "author": "someone",
            "summary": "One line.",
            "latest": {
                "version": "1.0",
                "file": file,
                "bytes": 10,
                "sha256": SHA,
                "min_rox": "2.0.0",
            },
        })
    }

    fn index(entries: Vec<Value>) -> String {
        serde_json::json!({ "version": 1, "generated": "2026-11-14T12:00:00Z", "packages": entries })
            .to_string()
    }

    fn ids(index: &Index) -> Vec<&str> {
        index
            .packages
            .iter()
            .map(|summary| summary.id.as_str())
            .collect()
    }

    #[test]
    fn an_index_reads() {
        let mut full = entry("aurora", "workspace", "files/aurora-1.2.0.roxw");
        full["tags"] = serde_json::json!(["dark", "minimal"]);
        full["license"] = "CC-BY-4.0".into();
        full["preview"] = "previews/aurora.webp".into();
        full["latest"]["requires"] = serde_json::json!(["some-plugin-id"]);

        let read = parse_index(&index(vec![
            full,
            entry("tones", "plugin", "files/tones-1.roxp"),
            entry("lamp", "object", "files/lamp.roxo"),
        ]))
        .unwrap();

        assert_eq!(ids(&read), vec!["aurora", "tones", "lamp"]);
        assert!(read.dropped.is_empty(), "{:?}", read.dropped);

        let aurora = read.get("aurora").unwrap();
        assert_eq!(aurora.kind, Kind::Workspace);
        assert_eq!(aurora.tags, vec!["dark", "minimal"]);
        assert_eq!(aurora.preview.as_deref(), Some("previews/aurora.webp"));
        assert_eq!(aurora.latest.requires, vec!["some-plugin-id"]);

        // The optional fields default.
        let tones = read.get("tones").unwrap();
        assert!(tones.tags.is_empty() && tones.license.is_empty() && tones.preview.is_none());
    }

    #[test]
    fn an_unknown_kind_is_dropped_and_the_rest_load() {
        let mut missing = entry("half", "plugin", "files/half.roxp");
        missing["latest"].as_object_mut().unwrap().remove("sha256");

        let read = parse_index(&index(vec![
            entry("aurora", "workspace", "files/aurora.roxw"),
            entry("strange", "theme-pack", "files/strange.zip"),
            missing,
            entry("lamp", "object", "files/lamp.roxo"),
            entry("lamp", "object", "files/lamp-again.roxo"),
        ]))
        .unwrap();

        assert_eq!(ids(&read), vec!["aurora", "lamp"]);
        assert_eq!(read.dropped.len(), 3, "{:?}", read.dropped);
        assert_eq!(read.get("lamp").unwrap().latest.file, "files/lamp.roxo");
    }

    #[test]
    fn an_absolute_file_url_costs_that_entry() {
        let mut preview = entry("pic", "object", "files/pic.roxo");
        preview["preview"] = "https://elsewhere.example/p.webp".into();

        let read = parse_index(&index(vec![
            entry("far", "workspace", "https://elsewhere.example/far.roxw"),
            entry("near", "workspace", "/v1/files/near.roxw"),
            entry("sneaky", "workspace", "//elsewhere.example/s.roxw"),
            entry("quoted", "workspace", "files/%2e%2e/q.roxw"),
            preview,
            entry("fine", "workspace", "files/fine.roxw"),
        ]))
        .unwrap();

        assert_eq!(ids(&read), vec!["fine"]);
        assert_eq!(read.dropped.len(), 5);
    }

    #[test]
    fn a_package_over_the_ceiling_costs_that_entry() {
        let mut over = entry("huge", "workspace", "files/huge.roxw");
        over["latest"]["bytes"] = (PACKAGE_MAX + 1).into();
        let mut at = entry("big", "workspace", "files/big.roxw");
        at["latest"]["bytes"] = PACKAGE_MAX.into();

        let read = parse_index(&index(vec![
            over,
            at,
            entry("fine", "workspace", "files/fine.roxw"),
        ]))
        .unwrap();

        assert_eq!(ids(&read), vec!["big", "fine"]);
        assert_eq!(read.dropped.len(), 1);
        assert!(read.dropped[0].contains("ceiling"), "{:?}", read.dropped);

        // A record's version is held to the same ceiling.
        let text = serde_json::json!({
            "id": "big",
            "versions": [
                { "version": "2", "file": "files/big-2.roxw", "bytes": PACKAGE_MAX + 1, "sha256": SHA, "min_rox": "2.0.0" },
                { "version": "1", "file": "files/big-1.roxw", "bytes": PACKAGE_MAX, "sha256": SHA, "min_rox": "2.0.0" },
            ],
        })
        .to_string();
        let record = parse_record("big", &text).unwrap();
        assert_eq!(record.versions.len(), 1);
        assert_eq!(record.versions[0].version, "1");
        assert_eq!(record.dropped.len(), 1);
    }

    #[test]
    fn a_parent_path_costs_that_entry() {
        let read = parse_index(&index(vec![
            entry("up", "workspace", "files/../../etc/passwd"),
            entry("dot", "workspace", "./files/dot.roxw"),
            entry("back", "workspace", "files\\back.roxw"),
            entry("fine", "workspace", "files/fine.roxw"),
        ]))
        .unwrap();

        assert_eq!(ids(&read), vec!["fine"]);
    }

    #[test]
    fn an_id_is_a_plain_slug() {
        for good in ["aurora", "a", "tones-2", "0day", &"a".repeat(64)] {
            assert!(is_slug(good), "{good}");
        }

        for bad in [
            "",
            "-flag",
            "Aurora",
            "a_b",
            "a.b",
            "a/b",
            "..",
            "a b",
            "über",
            "a\n",
            &"a".repeat(65),
        ] {
            assert!(!is_slug(bad), "{bad:?}");
        }

        // A bad id costs its entry and isn't echoed into the reason.
        let read = parse_index(&index(vec![entry(
            "../../x\nfake log",
            "plugin",
            "files/x.roxp",
        )]))
        .unwrap();
        assert!(read.packages.is_empty());
        assert!(!read.dropped[0].contains("fake log"), "{:?}", read.dropped);

        let origin = Origin::new("https://catalog.example").unwrap();
        assert!(fetch_record(&origin, "../index").is_err());
    }

    #[test]
    fn an_origin_is_a_bare_https_host() {
        assert!(Origin::new("https://catalog.example").is_ok());
        assert!(Origin::new("https://catalog.example:8443").is_ok());

        for bad in [
            "http://catalog.example",
            "https://catalog.example/",
            "https://catalog.example/v1",
            "https://user@catalog.example",
            "https://catalog.example?x",
            "ftp://catalog.example",
            "https://",
            "http://localhost:8000",
            "http://127.0.0.1.example",
        ] {
            assert!(Origin::new(bad).is_err(), "{bad}");
        }

        let origin = Origin::new("https://catalog.example").unwrap();
        assert_eq!(
            origin.resolve("files/a.roxw").unwrap(),
            "https://catalog.example/v1/files/a.roxw"
        );
        assert!(origin.resolve("https://elsewhere.example/a").is_err());
    }

    #[test]
    fn a_newer_layout_is_refused() {
        let text = serde_json::json!({ "version": 2, "packages": [] }).to_string();
        assert!(parse_index(&text).is_err());
    }

    #[test]
    fn a_record_for_another_id_is_refused() {
        let text = serde_json::json!({
            "id": "other",
            "versions": [],
        })
        .to_string();
        assert!(parse_record("aurora", &text).is_err());

        let text = serde_json::json!({
            "id": "aurora",
            "description": "Plain text.",
            "previews": ["previews/a.webp", "https://elsewhere.example/b.webp"],
            "versions": [
                { "version": "1.2.0", "file": "files/a-1.2.0.roxw", "bytes": 5, "sha256": SHA, "min_rox": "2.0.0" },
                { "version": "1.1.0", "file": "../a.roxw", "bytes": 5, "sha256": SHA, "min_rox": "2.0.0" },
            ],
        })
        .to_string();
        let record = parse_record("aurora", &text).unwrap();
        assert_eq!(record.previews, vec!["previews/a.webp"]);
        assert_eq!(record.versions.len(), 1);
        assert_eq!(record.dropped.len(), 2);
    }

    #[test]
    fn an_index_past_its_cap_is_refused() {
        let body = " ".repeat(INDEX_MAX as usize + 1);

        // Refused on the stated length, and on the count when there is none.
        for served in [canned::ok(&body), canned::open_ended(&body)] {
            let e = read_capped(canned::answer(&served), INDEX_MAX).unwrap_err();
            assert!(e.contains("cap"), "{e}");
        }

        let fits = " ".repeat(INDEX_MAX as usize);
        assert!(read_capped(canned::answer(&canned::open_ended(&fits)), INDEX_MAX).is_ok());
    }

    #[test]
    fn a_redirected_index_is_refused() {
        let e = read_capped(
            canned::answer(&canned::redirect("http://127.0.0.1:9/v1/index.json")),
            INDEX_MAX,
        )
        .unwrap_err();
        assert!(e.contains("redirect"), "{e}");

        assert!(format!("{:?}", agent()).contains("redirects: 0"));
    }

    #[test]
    fn a_preview_has_to_be_an_image() {
        let read = |served: String| image(read_capped(canned::answer(&served), PREVIEW_MAX)?);

        assert!(read(canned::typed("text/html", "<html></html>")).is_err());
        assert_eq!(read(canned::typed("image/png", "PNG")).unwrap(), b"PNG");
    }
}
