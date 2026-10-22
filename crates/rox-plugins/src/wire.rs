//! Wire v0: newline-delimited JSON-RPC 2.0 over the plugin's stdin and
//! stdout. The frame shape follows `rox-ipc`'s protocol, copied rather than
//! shared so the plugin wire and the control socket version apart.
//!
//! Everything a plugin sends is untrusted. Each result has its own struct
//! with `deny_unknown_fields`, strings are capped at [`MAX_STRING`] and lists
//! at [`MAX_ENTRIES`], and a frame that doesn't parse is the plugin's error,
//! never a panic. Conversion to library types happens in `rox-services`.

use std::collections::{BTreeMap, HashMap};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// The same cap as the control socket's frames.
pub const MAX_FRAME: usize = 1 << 20;

pub const MAX_STRING: usize = 4096;

/// Per page, for `entries` and `tracks` alike.
pub const MAX_ENTRIES: usize = 500;

/// So a base64 answer stays under [`MAX_FRAME`].
pub const MAX_READ: u32 = 512 * 1024;

#[derive(Serialize)]
pub struct Request<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    #[serde(skip_serializing_if = "Value::is_null")]
    pub params: Value,
}

impl Request<'_> {
    pub fn line(id: u64, method: &str, params: Value) -> Vec<u8> {
        let request = Request {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };

        // A `Value` tree with string keys always serializes.
        let mut line = serde_json::to_vec(&request).unwrap_or_default();
        line.push(b'\n');

        line
    }
}

/// A plugin's answer. `result: null` is a real answer (`source.close`,
/// `shutdown`, a cover it doesn't have), so a present null stays apart from a
/// missing field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    #[serde(default)]
    pub jsonrpc: Option<String>,
    pub id: Value,
    #[serde(default, deserialize_with = "present")]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    /// JSON-RPC allows it; the host has no use for it.
    #[serde(default)]
    pub data: Option<Value>,
}

/// One inbound line, sorted into what the host acts on.
#[derive(Debug, PartialEq)]
pub enum Inbound {
    Answer {
        id: u64,
        result: Result<Value, String>,
    },
    /// Not a frame the host can pair with anything. Logged, then dropped.
    Junk(String),
}

pub fn parse_line(line: &[u8]) -> Inbound {
    let response: Response = match serde_json::from_slice(line) {
        Ok(response) => response,
        Err(e) => return Inbound::Junk(format!("unparseable frame: {e}")),
    };

    if response.jsonrpc.as_deref().is_some_and(|v| v != "2.0") {
        return Inbound::Junk("jsonrpc is not \"2.0\"".into());
    }

    let Some(id) = response.id.as_u64() else {
        return Inbound::Junk(format!(
            "an answer to id {} the host never sent",
            response.id
        ));
    };

    let result = match (response.result, response.error) {
        (Some(value), None) => Ok(value),
        (None, Some(error)) => Err(clip(&error.message).to_string()),
        (Some(_), Some(_)) => Err("the plugin answered with a result and an error".into()),
        (None, None) => Err("the plugin answered with neither a result nor an error".into()),
    };

    Inbound::Answer { id, result }
}

/// Error text from a plugin reaches the log and the UI, so it's cut short.
fn clip(text: &str) -> &str {
    if text.len() <= MAX_STRING {
        return text;
    }

    let mut end = MAX_STRING;
    while !text.is_char_boundary(end) {
        end -= 1;
    }

    &text[..end]
}

/// A result's own checks past what serde enforces.
pub trait Checked: Sized {
    fn check(&self) -> Result<(), String>;
}

/// The typed form of a result, checked.
pub fn decode<T: for<'de> Deserialize<'de> + Checked>(value: Value) -> Result<T, String> {
    let typed: T = serde_json::from_value(value).map_err(|e| format!("malformed result: {e}"))?;
    typed.check()?;

    Ok(typed)
}

fn string(field: &str, value: &str) -> Result<(), String> {
    match value.len() > MAX_STRING {
        true => Err(format!("{field} is longer than {MAX_STRING} bytes")),
        false => Ok(()),
    }
}

fn confidence(value: Option<f32>) -> Result<(), String> {
    match value {
        Some(value) if !(0.0..=1.0).contains(&value) => {
            Err(format!("a confidence of {value} is outside 0 to 1"))
        }
        _ => Ok(()),
    }
}

/// A click hands a notice's link to the OS, which opens any scheme it has a
/// handler for, `file:` included. Only a web address gets that far.
fn web_url(field: &str, url: &str) -> Result<(), String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    let clean = !url.chars().any(|c| c.is_whitespace() || c.is_control());

    match rest {
        Some(rest) if clean && !rest.is_empty() && !rest.starts_with('/') => Ok(()),
        _ => Err(format!("{field} isn't an http or https URL")),
    }
}

fn list<T>(field: &str, items: &[T]) -> Result<(), String> {
    match items.len() > MAX_ENTRIES {
        true => Err(format!("{field} holds more than {MAX_ENTRIES} items")),
        false => Ok(()),
    }
}

/// A row's flags, the `flags` feature: None when the plugin doesn't know them.
fn flags(value: &Option<Vec<String>>) -> Result<(), String> {
    let Some(flags) = value else {
        return Ok(());
    };

    if flags.len() > MAX_FLAGS {
        return Err(format!("a row has more than {MAX_FLAGS} flags"));
    }

    flags.iter().try_for_each(|flag| string("flag", flag))
}

fn cursor(value: &Option<String>) -> Result<(), String> {
    value.as_deref().map_or(Ok(()), |c| string("cursor", c))
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub name: String,
    pub version: String,
    pub api: u32,
}

impl Checked for Hello {
    fn check(&self) -> Result<(), String> {
        string("name", &self.name)?;
        string("version", &self.version)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub notice: Option<Notice>,
    /// Other ways to list this place, the `views` feature. Read from a
    /// place's first page.
    #[serde(default)]
    pub views: Vec<View>,
    /// Which of `views` this page is.
    #[serde(default)]
    pub view: Option<String>,
    /// Extra columns the entries carry values for, the `fields` feature.
    #[serde(default)]
    pub fields: Vec<Field>,
}

/// A column the service knows and a row's tags don't, like a popularity.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
}

/// How a field's values read and sort.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    /// A whole number, shown short (12.3k).
    Count,
    /// 0 to 100.
    Percent,
    /// `YYYY-MM-DD`, or any prefix of it.
    Date,
    Text,
}

/// A row's values by field id: numbers for counts and percents, strings for
/// dates and text.
pub type Values = BTreeMap<String, Value>;

fn values(field: &str, values: &Values) -> Result<(), String> {
    for (id, value) in values {
        string(field, id)?;
        match value {
            Value::Number(_) => {}
            Value::String(text) => string(field, text)?,
            _ => return Err(format!("{field} {id} is neither a number nor a string")),
        }
    }

    Ok(())
}

/// One way to list a place: a filter or an order the service applies.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub id: String,
    pub label: String,
}

/// The optional parts of API 1 this host reads, sent in `hello`. A host
/// from before one refuses a result that uses it, so a plugin checks here.
pub const FEATURES: &[&str] = &[
    "notice",
    "notice-link",
    "node-kind",
    "node-art",
    "sections",
    "views",
    "fields",
    "tiles",
    "home",
    "open-duration",
    "go-to",
    "flags",
    "chapters",
];

/// More than a seek strip can tell apart.
pub const MAX_CHAPTERS: usize = 500;

/// A whole sheet in one answer: an enhanced LRC runs to tens of KB, far past
/// [`MAX_STRING`], and this stays well under [`MAX_FRAME`].
pub const MAX_LYRICS: usize = 256 * 1024;

/// More than a picker shows; a plugin ranks its own before it answers.
pub const MAX_CANDIDATES: usize = 50;

/// A bio runs to paragraphs, past [`MAX_STRING`].
pub const MAX_BIO: usize = 64 * 1024;

/// More flags than a row has states worth gating an action on.
pub const MAX_FLAGS: usize = 16;

/// More than a row of chips holds.
pub const MAX_VIEWS: usize = 12;

/// Columns beside a row's title, which has to keep most of the width.
pub const MAX_FIELDS: usize = 4;

/// A line the plugin wants shown over a page, like a setting it needs
/// before it can list anything.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Notice {
    pub text: String,
    #[serde(default)]
    pub kind: NoticeKind,
    #[serde(default)]
    pub link: Option<NoticeLink>,
}

/// A web page the notice points at, opened in the browser on a click. The
/// `notice-link` feature.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoticeLink {
    pub url: String,
    /// The button's label. Empty reads as the host's own.
    #[serde(default)]
    pub label: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NoticeKind {
    /// Something worth knowing about the page.
    #[default]
    Info,
    /// The plugin needs a setting from the user, so rox offers its settings.
    Setup,
}

impl Checked for Page {
    fn check(&self) -> Result<(), String> {
        list("entries", &self.entries)?;
        cursor(&self.cursor)?;
        if let Some(notice) = &self.notice {
            if notice.text.trim().is_empty() {
                return Err("a notice has no text".into());
            }
            string("notice", &notice.text)?;

            if let Some(link) = &notice.link {
                string("notice link", &link.url)?;
                string("notice link label", &link.label)?;
                web_url("a notice link", &link.url)?;
            }
        }

        if self.views.len() > MAX_VIEWS {
            return Err(format!("a page offers more than {MAX_VIEWS} views"));
        }

        for view in &self.views {
            if view.id.is_empty() || view.label.trim().is_empty() {
                return Err("a view has an empty id or label".into());
            }
            string("view id", &view.id)?;
            string("view label", &view.label)?;
        }

        if let Some(view) = &self.view
            && !self.views.iter().any(|v| &v.id == view)
        {
            return Err(format!("the page is view {view:?}, which it doesn't offer"));
        }

        if self.fields.len() > MAX_FIELDS {
            return Err(format!("a page declares more than {MAX_FIELDS} fields"));
        }

        for (ix, field) in self.fields.iter().enumerate() {
            if field.id.is_empty() || field.label.trim().is_empty() {
                return Err("a field has an empty id or label".into());
            }
            if self.fields[..ix].iter().any(|f| f.id == field.id) {
                return Err(format!("field {:?} is declared twice", field.id));
            }
            string("field id", &field.id)?;
            string("field label", &field.label)?;
        }

        // A value for a field the page never declared has no column to go in.
        let declared = |row: &Values| {
            row.keys()
                .find(|id| !self.fields.iter().any(|f| &f.id == *id))
                .map_or(Ok(()), |id| {
                    Err(format!(
                        "a value for field {id:?}, which the page doesn't declare"
                    ))
                })
        };

        self.entries.iter().try_for_each(|entry| match entry {
            Entry::Node(node) => node.check().and_then(|()| declared(&node.values)),
            Entry::Track(track) => track.check().and_then(|()| declared(&track.values)),
            Entry::Section(section) => section.check(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Entry {
    Node(Node),
    Track(Track),
    /// A heading over the entries after it, the `sections` feature.
    Section(Section),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub title: String,
    /// How the entries under it show, the `tiles` feature.
    #[serde(default)]
    pub layout: Layout,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Rows,
    /// A shelf of cover tiles scrolled sideways, for albums, playlists and
    /// the like. Tracks read better as rows.
    Tiles,
}

impl Checked for Section {
    fn check(&self) -> Result<(), String> {
        if self.title.trim().is_empty() {
            return Err("a section has no title".into());
        }

        string("section title", &self.title)
    }
}

/// What a node is, for its icon: the `node-kind` feature.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Album,
    Playlist,
    Artist,
    Folder,
}

impl NodeKind {
    /// The wire's name for it, which settings store.
    pub fn name(self) -> &'static str {
        match self {
            NodeKind::Album => "album",
            NodeKind::Playlist => "playlist",
            NodeKind::Artist => "artist",
            NodeKind::Folder => "folder",
        }
    }

    pub fn named(name: &str) -> Option<NodeKind> {
        [
            NodeKind::Album,
            NodeKind::Playlist,
            NodeKind::Artist,
            NodeKind::Folder,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    #[serde(default)]
    pub collection: bool,
    #[serde(default)]
    pub kind: Option<NodeKind>,
    /// A key for `source.cover`, the `node-art` feature. Empty for none.
    #[serde(default)]
    pub art: String,
    #[serde(default)]
    pub values: Values,
    /// The service's home among the roots, the `home` feature. rox can list
    /// its page under the roots instead of as a node of its own.
    #[serde(default)]
    pub home: bool,
    /// What the node is right now, for actions' `when`: the `flags`
    /// feature. None when the plugin doesn't know.
    #[serde(default)]
    pub flags: Option<Vec<String>>,
}

impl Checked for Node {
    fn check(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("a node has an empty id".into());
        }

        string("node id", &self.id)?;
        string("title", &self.title)?;
        string("subtitle", &self.subtitle)?;
        string("node art", &self.art)?;
        flags(&self.flags)?;
        values("node value", &self.values)
    }
}

/// Every field but `key` defaults, empty or zero: the contract says a plugin
/// always sends them, and a missing one is cheaper to forgive than to refuse.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Track {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album_artist: String,
    pub album: String,
    pub genre: String,
    pub year: u16,
    pub disc_no: u16,
    pub track_no: u16,
    pub duration_ms: u32,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub live: bool,
    /// The `fields` feature's values. Sync ignores them: a kept row holds
    /// only its tags.
    pub values: Values,
    /// The `go-to` feature. Sync keeps it beside the row, so a kept row
    /// offers Go to too. Boxed, since most tracks leave it out.
    pub go_to: Option<Box<GoTo>>,
    /// The `flags` feature, as on a node. Sync ignores them: they go stale.
    pub flags: Option<Vec<String>>,
}

impl Checked for Track {
    fn check(&self) -> Result<(), String> {
        if self.key.is_empty() {
            return Err("a track has an empty key".into());
        }

        [
            ("key", &self.key),
            ("title", &self.title),
            ("artist", &self.artist),
            ("album_artist", &self.album_artist),
            ("album", &self.album),
            ("genre", &self.genre),
            ("codec", &self.codec),
        ]
        .into_iter()
        .try_for_each(|(field, value)| string(field, value))?;

        values("track value", &self.values)?;
        flags(&self.flags)?;
        self.go_to.as_ref().map_or(Ok(()), |go_to| go_to.check())
    }
}

/// The nodes a track's album and artists open, for Go to in the source
/// browser.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GoTo {
    pub album: Option<Node>,
    pub artists: Vec<Node>,
}

impl Checked for GoTo {
    fn check(&self) -> Result<(), String> {
        list("go_to artists", &self.artists)?;

        self.album
            .iter()
            .chain(&self.artists)
            .try_for_each(Node::check)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SyncPage {
    #[serde(default)]
    pub unchanged: bool,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

/// A batch of a radio seeded from a track or a node, the `radio` capability.
/// A null `cursor` means the station ran out; the host seeds a new one.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RadioPage {
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub cursor: Option<String>,
}

impl Checked for RadioPage {
    fn check(&self) -> Result<(), String> {
        list("tracks", &self.tracks)?;
        cursor(&self.cursor)?;

        self.tracks.iter().try_for_each(Track::check)
    }
}

impl Checked for SyncPage {
    fn check(&self) -> Result<(), String> {
        list("tracks", &self.tracks)?;
        cursor(&self.cursor)?;
        self.token
            .as_deref()
            .map_or(Ok(()), |t| string("token", t))?;

        self.tracks.iter().try_for_each(Track::check)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Open {
    pub stream: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub length: Option<u64>,
    #[serde(default)]
    pub seekable: bool,
    #[serde(default)]
    pub live: bool,
    /// Absent reads as whole.
    #[serde(default)]
    pub buffer: Option<Buffer>,
    /// The stream's length, for a container that doesn't state one. Only
    /// sent once `hello` listed `open-duration`.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Where the stream's parts start, the `chapters` feature.
    #[serde(default)]
    pub chapters: Vec<Chapter>,
}

/// One part of a long stream, like an episode's segment.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Chapter {
    pub start_ms: u64,
    pub title: String,
}

/// How much of a stream the host may fetch ahead. A plugin can only lower it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Buffer {
    Whole,
    Ahead,
}

impl Checked for Open {
    fn check(&self) -> Result<(), String> {
        if self.stream.is_empty() {
            return Err("an open answered an empty stream id".into());
        }

        string("stream", &self.stream)?;
        string("hint", &self.hint)?;

        // The order and the titles' content aren't checked here: a bad
        // chapter list mustn't stop the track playing, so services tidy it.
        if self.chapters.len() > MAX_CHAPTERS {
            return Err(format!("an open has more than {MAX_CHAPTERS} chapters"));
        }

        self.chapters
            .iter()
            .try_for_each(|chapter| string("chapter title", &chapter.title))
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Read {
    pub data: String,
}

impl Checked for Read {
    fn check(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Read {
    /// The bytes, refused past what was asked for.
    pub fn bytes(&self, asked: u32) -> Result<Vec<u8>, String> {
        let bytes = BASE64
            .decode(&self.data)
            .map_err(|e| format!("read data is not base64: {e}"))?;

        match bytes.len() > asked as usize {
            true => Err(format!(
                "a read answered {} bytes for {asked} asked",
                bytes.len()
            )),
            false => Ok(bytes),
        }
    }
}

/// A track's or a node's web page, `source.link`'s answer. rox only opens it
/// in the browser or copies it, on the user's click, and never fetches it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub url: String,
}

impl Checked for Link {
    fn check(&self) -> Result<(), String> {
        string("link", &self.url)?;
        web_url("a link", &self.url)
    }
}

/// A track's lyrics, `source.lyrics`'s answer: LRC when `synced`, plain
/// lines otherwise.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LyricsAnswer {
    pub text: String,
    #[serde(default)]
    pub synced: bool,
}

impl Checked for LyricsAnswer {
    fn check(&self) -> Result<(), String> {
        if self.text.trim().is_empty() {
            return Err("lyrics with no text; answer null for none".into());
        }

        match self.text.len() > MAX_LYRICS {
            true => Err(format!("lyrics longer than {MAX_LYRICS} bytes")),
            false => Ok(()),
        }
    }
}

fn candidates<T>(items: &[T]) -> Result<(), String> {
    match items.len() > MAX_CANDIDATES {
        true => Err(format!("more than {MAX_CANDIDATES} candidates")),
        false => Ok(()),
    }
}

/// `provider.lyrics`'s answer: sheets for a track the plugin found by its tags.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LyricsCandidates {
    pub candidates: Vec<LyricsHit>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LyricsHit {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: Option<f64>,
    /// LRC when `synced`, plain lines otherwise. Never blank.
    pub text: String,
    pub synced: bool,
    /// The plugin's own view of the match, 0 to 1. rox blends it in; see
    /// `blend` in rox-net.
    pub confidence: Option<f32>,
}

impl Checked for LyricsCandidates {
    fn check(&self) -> Result<(), String> {
        candidates(&self.candidates)?;

        for hit in &self.candidates {
            if hit.text.trim().is_empty() {
                return Err("a candidate with no text".into());
            }
            if hit.text.len() > MAX_LYRICS {
                return Err(format!("a candidate longer than {MAX_LYRICS} bytes"));
            }

            string("artist", &hit.artist)?;
            string("title", &hit.title)?;
            string("album", &hit.album)?;
            confidence(hit.confidence)?;
        }

        Ok(())
    }
}

/// `provider.metadata`'s answer. A field the service lacks stays empty and
/// the tag compare leaves it alone, the same as a built-in's.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MetadataCandidates {
    pub candidates: Vec<MetadataHit>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct MetadataHit {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub artist_sort: String,
    pub album_artist_sort: String,
    pub year: String,
    pub track_no: String,
    pub disc_no: String,
    pub duration_secs: Option<f64>,
    pub confidence: Option<f32>,
}

impl Checked for MetadataCandidates {
    fn check(&self) -> Result<(), String> {
        candidates(&self.candidates)?;

        for hit in &self.candidates {
            for (field, value) in [
                ("title", &hit.title),
                ("artist", &hit.artist),
                ("album", &hit.album),
                ("album_artist", &hit.album_artist),
                ("artist_sort", &hit.artist_sort),
                ("album_artist_sort", &hit.album_artist_sort),
                ("year", &hit.year),
                ("track_no", &hit.track_no),
                ("disc_no", &hit.disc_no),
            ] {
                string(field, value)?;
            }

            confidence(hit.confidence)?;
        }

        Ok(())
    }
}

/// `provider.artist`'s answer: text only. No image reaches rox from a
/// plugin as a URL (ADR 30), so pictures stay with the built-in providers.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct ArtistSheet {
    /// Plain paragraphs separated by blank lines. Never blank.
    pub bio: String,
    /// The artist's page on the service, or empty.
    pub url: String,
    pub tags: Vec<String>,
    pub similar: Vec<String>,
    /// ISO 3166 alpha-2, uppercase, or empty.
    pub country: String,
    pub formed: Option<u16>,
    pub ended: Option<u16>,
}

impl Checked for ArtistSheet {
    fn check(&self) -> Result<(), String> {
        if self.bio.trim().is_empty() {
            return Err("a sheet with no bio; answer null for none".into());
        }
        if self.bio.len() > MAX_BIO {
            return Err(format!("a bio longer than {MAX_BIO} bytes"));
        }

        if !self.url.is_empty() {
            string("url", &self.url)?;
            web_url("an artist url", &self.url)?;
        }

        for (field, items) in [("tags", &self.tags), ("similar", &self.similar)] {
            list(field, items)?;
            for item in items {
                string(field, item)?;
            }
        }

        let alpha2 =
            self.country.len() == 2 && self.country.bytes().all(|b| b.is_ascii_uppercase());
        if !self.country.is_empty() && !alpha2 {
            return Err("a country that isn't two uppercase letters".into());
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Cover {
    pub mime: String,
    pub data: String,
}

impl Checked for Cover {
    fn check(&self) -> Result<(), String> {
        string("mime", &self.mime)
    }
}

impl Cover {
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        BASE64
            .decode(&self.data)
            .map_err(|e| format!("cover data is not base64: {e}"))
    }
}

/// What an action left for the user when it ended. rox shows `message` in
/// a toast, and the link and the path become its buttons and nothing more:
/// the link opens in the browser and the path shows in the file manager,
/// each only on a click. rox never fetches the link or opens the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub message: String,
    pub link: Option<String>,
    pub reveal: Option<String>,
}

impl Outcome {
    fn check(&self) -> Result<(), String> {
        string("message", &self.message)?;

        if let Some(link) = &self.link {
            string("link", link)?;
            web_url("an outcome's link", link)?;
        }

        if let Some(path) = &self.reveal {
            string("reveal", path)?;

            let clean = !path.chars().any(|c| c.is_control());
            if !clean || !std::path::Path::new(path).is_absolute() {
                return Err("reveal isn't an absolute path".into());
            }
        }

        Ok(())
    }
}

/// `source.action`'s answer: finished at once, or a job rox polls.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionAnswer {
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub reveal: Option<String>,
    /// The items' flags now that the action ran, the `flags` feature.
    #[serde(default)]
    pub flags: HashMap<String, Vec<String>>,
}

impl ActionAnswer {
    pub fn outcome(&self) -> Outcome {
        Outcome {
            message: self.message.clone(),
            link: self.link.clone(),
            reveal: self.reveal.clone(),
        }
    }
}

impl Checked for ActionAnswer {
    fn check(&self) -> Result<(), String> {
        if let Some(job) = &self.job {
            string("job", job)?;
            if job.is_empty() {
                return Err("job is empty".into());
            }
        }

        changed_flags(&self.flags)?;
        self.outcome().check()
    }
}

/// An action's or job's flags for the items it touched.
fn changed_flags(changed: &HashMap<String, Vec<String>>) -> Result<(), String> {
    if changed.len() > MAX_ENTRIES {
        return Err(format!("flags name more than {MAX_ENTRIES} items"));
    }

    changed.iter().try_for_each(|(item, now)| {
        string("flagged item", item)?;
        flags(&Some(now.clone()))
    })
}

/// `source.flags`'s answer: what each item asked about is right now. An
/// item left out stays unknown.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FlagsAnswer {
    pub flags: HashMap<String, Vec<String>>,
}

impl Checked for FlagsAnswer {
    fn check(&self) -> Result<(), String> {
        changed_flags(&self.flags)
    }
}

/// `source.job`'s answer, polled about once a second. An `error` ends the
/// job as failed; `finished` ends it with the outcome fields.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct JobState {
    #[serde(default)]
    pub done: u64,
    /// Zero when the plugin can't tell how much there is.
    #[serde(default)]
    pub total: u64,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub finished: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub reveal: Option<String>,
    /// As on an action's answer, read once the job finishes.
    #[serde(default)]
    pub flags: HashMap<String, Vec<String>>,
}

impl JobState {
    pub fn outcome(&self) -> Outcome {
        Outcome {
            message: self.message.clone(),
            link: self.link.clone(),
            reveal: self.reveal.clone(),
        }
    }
}

impl Checked for JobState {
    fn check(&self) -> Result<(), String> {
        string("text", &self.text)?;
        if let Some(error) = &self.error {
            string("error", error)?;
        }

        changed_flags(&self.flags)?;
        self.outcome().check()
    }
}

/// `null` answers: close, shutdown.
pub fn is_null(value: &Value) -> Result<(), String> {
    match value.is_null() {
        true => Ok(()),
        false => Err(format!("expected null, got {value}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_node_kind_round_trips_through_its_name() {
        for kind in [
            NodeKind::Album,
            NodeKind::Playlist,
            NodeKind::Artist,
            NodeKind::Folder,
        ] {
            assert_eq!(NodeKind::named(kind.name()), Some(kind));
            assert_eq!(
                serde_json::from_value::<NodeKind>(json!(kind.name())).unwrap(),
                kind,
                "settings store the wire's own name"
            );
        }

        assert_eq!(NodeKind::named(""), None);
    }

    #[test]
    fn a_request_is_one_line_with_its_id() {
        let line = Request::line(
            7,
            "source.read",
            json!({"stream": "s1", "offset": 0, "len": 4}),
        );

        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);

        let back: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(back["jsonrpc"], "2.0");
        assert_eq!(back["id"], 7);
        assert_eq!(back["method"], "source.read");
    }

    #[test]
    fn a_request_with_no_params_leaves_the_field_out() {
        let back: Value =
            serde_json::from_slice(&Request::line(1, "shutdown", Value::Null)).unwrap();
        assert!(back.get("params").is_none());
    }

    #[test]
    fn a_null_result_is_an_answer() {
        let inbound = parse_line(br#"{"jsonrpc":"2.0","id":3,"result":null}"#);
        assert_eq!(
            inbound,
            Inbound::Answer {
                id: 3,
                result: Ok(Value::Null)
            }
        );
    }

    #[test]
    fn neither_result_nor_error_is_the_plugins_error() {
        let inbound = parse_line(br#"{"jsonrpc":"2.0","id":3}"#);
        assert!(matches!(
            inbound,
            Inbound::Answer {
                id: 3,
                result: Err(_)
            }
        ));
    }

    #[test]
    fn an_error_answer_carries_its_message() {
        let inbound = parse_line(
            br#"{"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"no such video","data":{"x":1}}}"#,
        );
        assert_eq!(
            inbound,
            Inbound::Answer {
                id: 4,
                result: Err("no such video".into())
            }
        );
    }

    #[test]
    fn the_jsonrpc_field_is_optional_but_checked() {
        assert!(matches!(
            parse_line(br#"{"id":1,"result":1}"#),
            Inbound::Answer { id: 1, .. }
        ));
        assert!(matches!(
            parse_line(br#"{"jsonrpc":"1.0","id":1,"result":1}"#),
            Inbound::Junk(_)
        ));
    }

    #[test]
    fn junk_never_panics() {
        for line in [
            &b"not json"[..],
            br#"{"id":1,"result":1,"extra":true}"#,
            br#"{"id":"one","result":1}"#,
            br#"{"id":null,"error":{"code":1,"message":"x"}}"#,
            br#"[]"#,
            b"",
        ] {
            assert!(matches!(parse_line(line), Inbound::Junk(_)), "{line:?}");
        }
    }

    #[test]
    fn a_page_holds_nodes_and_tracks() {
        let page: Page = decode(json!({
            "entries": [
                {"node": {"id": "likes", "title": "Likes", "subtitle": "", "collection": true}},
                {"track": {"key": "k1", "title": "One", "duration_ms": 1000}}
            ],
            "cursor": null
        }))
        .unwrap();

        assert_eq!(page.entries.len(), 2);
        assert!(
            matches!(&page.entries[1], Entry::Track(t) if t.key == "k1" && t.artist.is_empty())
        );
    }

    #[test]
    fn a_page_carries_a_notice() {
        let page: Page = decode(json!({
            "entries": [],
            "notice": {"text": "Set a cookies file", "kind": "setup"}
        }))
        .unwrap();
        let notice = page.notice.expect("the notice parses");
        assert_eq!(notice.kind, NoticeKind::Setup);

        let plain: Page = decode(json!({"entries": [], "notice": {"text": "hi"}})).unwrap();
        assert_eq!(plain.notice.map(|n| n.kind), Some(NoticeKind::Info));

        let linked: Page = decode(json!({
            "entries": [],
            "notice": {"text": "Sign in", "link": {"url": "https://example.com/abc", "label": "Sign In"}}
        }))
        .unwrap();
        let link = linked.notice.and_then(|n| n.link).expect("the link parses");
        assert_eq!(
            (link.url.as_str(), link.label.as_str()),
            ("https://example.com/abc", "Sign In")
        );

        for bad in [
            json!({"entries": [], "notice": {"text": " "}}),
            json!({"entries": [], "notice": {"text": "x", "kind": "urgent"}}),
            json!({"entries": [], "notice": {"text": "x", "link": "y"}}),
            json!({"entries": [], "notice": {"text": "x", "link": {"url": "https://a.b", "extra": 1}}}),
        ] {
            assert!(decode::<Page>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn nodes_carry_a_kind_and_art_and_sections_head_them() {
        let page: Page = decode(json!({
            "entries": [
                {"section": {"title": "Albums"}},
                {"node": {"id": "album:1", "title": "A", "kind": "album", "art": "album:1"}},
                {"node": {"id": "plain", "title": "B"}}
            ]
        }))
        .unwrap();

        assert!(
            matches!(&page.entries[0], Entry::Section(s) if s.title == "Albums" && s.layout == Layout::Rows)
        );

        let shelf: Page =
            decode(json!({"entries": [{"section": {"title": "New", "layout": "tiles"}}]})).unwrap();
        assert!(matches!(&shelf.entries[0], Entry::Section(s) if s.layout == Layout::Tiles));
        assert!(
            decode::<Page>(json!({"entries": [{"section": {"title": "New", "layout": "cards"}}]}))
                .is_err()
        );
        assert!(
            matches!(&page.entries[1], Entry::Node(n) if n.kind == Some(NodeKind::Album) && n.art == "album:1")
        );
        assert!(
            matches!(&page.entries[2], Entry::Node(n) if n.kind.is_none() && n.art.is_empty() && !n.home)
        );

        let roots: Page =
            decode(json!({"entries": [{"node": {"id": "home", "title": "Home", "home": true}}]}))
                .unwrap();
        assert!(matches!(&roots.entries[0], Entry::Node(n) if n.home));

        for bad in [
            json!({"entries": [{"section": {"title": " "}}]}),
            json!({"entries": [{"node": {"id": "x", "title": "x", "kind": "genre"}}]}),
        ] {
            assert!(decode::<Page>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_track_names_the_nodes_its_album_and_artists_open() {
        let page: Page = decode(json!({
            "entries": [{"track": {"key": "t1", "go_to": {
                "album": {"id": "album:1", "title": "A", "kind": "album", "collection": true},
                "artists": [
                    {"id": "artist:1", "title": "B", "kind": "artist"},
                    {"id": "artist:2", "title": "C", "kind": "artist"}
                ]
            }}}]
        }))
        .unwrap();

        let Entry::Track(track) = &page.entries[0] else {
            panic!("not a track");
        };
        let go_to = track.go_to.as_ref().unwrap();
        assert_eq!(go_to.album.as_ref().unwrap().id, "album:1");
        assert_eq!(go_to.artists.len(), 2);

        let bare: Page = decode(json!({"entries": [{"track": {"key": "t2"}}]})).unwrap();
        assert!(matches!(&bare.entries[0], Entry::Track(t) if t.go_to.is_none()));

        for bad in [
            json!({"entries": [{"track": {"key": "t", "go_to": {"album": {"id": "", "title": "A"}}}}]}),
            json!({"entries": [{"track": {"key": "t", "go_to": {"label": "A"}}}]}),
        ] {
            assert!(decode::<Page>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_page_offers_views_and_names_its_own() {
        let page: Page = decode(json!({
            "entries": [],
            "views": [{"id": "all", "label": "All"}, {"id": "albums", "label": "Albums"}],
            "view": "albums"
        }))
        .unwrap();
        assert_eq!(page.views.len(), 2);
        assert_eq!(page.view.as_deref(), Some("albums"));

        let many: Vec<Value> = (0..=MAX_VIEWS)
            .map(|i| json!({"id": i.to_string(), "label": "v"}))
            .collect();

        for bad in [
            json!({"entries": [], "views": [{"id": "all", "label": "All"}], "view": "gone"}),
            json!({"entries": [], "views": [{"id": "", "label": "All"}]}),
            json!({"entries": [], "views": many}),
        ] {
            assert!(decode::<Page>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn fields_declare_the_columns_rows_carry_values_for() {
        let page: Page = decode(json!({
            "fields": [{"id": "pop", "label": "Popularity", "kind": "percent"}],
            "entries": [
                {"track": {"key": "1", "values": {"pop": 78}}},
                {"node": {"id": "n", "title": "N", "values": {"pop": 40}}},
                {"track": {"key": "2"}}
            ]
        }))
        .unwrap();
        assert_eq!(page.fields[0].kind, FieldKind::Percent);
        assert!(matches!(&page.entries[0], Entry::Track(t) if t.values["pop"] == 78));

        let five: Vec<Value> = (0..=MAX_FIELDS)
            .map(|i| json!({"id": i.to_string(), "label": "f", "kind": "count"}))
            .collect();

        for bad in [
            json!({"entries": [{"track": {"key": "1", "values": {"pop": 1}}}]}),
            json!({"fields": [{"id": "a", "label": "A", "kind": "count"}], "entries": [{"track": {"key": "1", "values": {"a": [1]}}}]}),
            json!({"fields": [{"id": "a", "label": "A", "kind": "rating"}], "entries": []}),
            json!({"fields": [{"id": "a", "label": "A", "kind": "count"}, {"id": "a", "label": "B", "kind": "count"}], "entries": []}),
            json!({"fields": five, "entries": []}),
        ] {
            assert!(decode::<Page>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_radio_page_is_tracks_and_a_cursor() {
        let page: RadioPage =
            decode(json!({"tracks": [{"key": "1"}, {"key": "2"}], "cursor": "2"})).unwrap();
        assert_eq!(page.tracks.len(), 2);
        assert_eq!(page.cursor.as_deref(), Some("2"));

        assert!(decode::<RadioPage>(json!({"tracks": [{"key": ""}]})).is_err());
        assert!(decode::<RadioPage>(json!({"tracks": [], "station": "x"})).is_err());
    }

    #[test]
    fn a_notice_link_is_only_a_web_address() {
        for url in ["https://example.com", "http://127.0.0.1:8080/x?y=z"] {
            assert!(web_url("a link", url).is_ok(), "{url}");
        }

        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ms-settings:privacy",
            "https://",
            "https:///path",
            "HTTPS://example.com",
            "https://example.com/a b",
            "https://example.com/\n",
        ] {
            assert!(web_url("a link", url).is_err(), "{url}");
        }
    }

    #[test]
    fn caps_and_empty_ids_are_refused() {
        let long = "x".repeat(MAX_STRING + 1);

        let err = decode::<Page>(json!({"entries": [{"track": {"key": long}}]})).unwrap_err();
        assert!(err.contains("key is longer"), "{err}");

        let err =
            decode::<Page>(json!({"entries": [{"node": {"id": "", "title": "t"}}]})).unwrap_err();
        assert!(err.contains("empty id"), "{err}");

        let many: Vec<Value> = (0..=MAX_ENTRIES)
            .map(|i| json!({"key": format!("k{i}")}))
            .collect();
        let err = decode::<SyncPage>(json!({"tracks": many})).unwrap_err();
        assert!(err.contains("more than 500"), "{err}");

        let err =
            decode::<Page>(json!({"entries": [{"track": {"key": "k", "rating": 5}}]})).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn a_read_refuses_more_than_it_asked_for() {
        let read: Read = decode(json!({"data": BASE64.encode([1u8, 2, 3, 4])})).unwrap();

        assert_eq!(read.bytes(4).unwrap(), vec![1, 2, 3, 4]);
        assert!(read.bytes(3).is_err());

        let bad: Read = decode(json!({"data": "***"})).unwrap();
        assert!(bad.bytes(10).is_err());
    }

    #[test]
    fn an_open_names_its_stream() {
        let open: Open = decode(
            json!({"stream": "s1", "hint": "m4a", "length": 10, "seekable": true, "live": false}),
        )
        .unwrap();
        assert_eq!(open.length, Some(10));

        assert!(decode::<Open>(json!({"stream": ""})).is_err());

        let ahead: Open = decode(json!({"stream": "s1", "buffer": "ahead"})).unwrap();
        assert_eq!(ahead.buffer, Some(Buffer::Ahead));
        assert!(decode::<Open>(json!({"stream": "s1", "buffer": "everything"})).is_err());
    }

    #[test]
    fn an_open_may_name_its_chapters() {
        let open: Open = decode(json!({
            "stream": "s1",
            "chapters": [{"start_ms": 0, "title": "Intro"}, {"start_ms": 271_000, "title": "Story"}],
        }))
        .unwrap();
        assert_eq!(open.chapters[1].start_ms, 271_000);

        let plain: Open = decode(json!({"stream": "s1"})).unwrap();
        assert!(plain.chapters.is_empty());

        let many: Vec<_> = (0..=MAX_CHAPTERS as u64)
            .map(|i| json!({"start_ms": i, "title": "x"}))
            .collect();
        for bad in [
            json!({"stream": "s1", "chapters": many}),
            json!({"stream": "s1", "chapters": [{"start_ms": 0}]}),
            json!({"stream": "s1", "chapters": [{"start_ms": 0, "title": "x".repeat(MAX_STRING + 1)}]}),
            json!({"stream": "s1", "chapters": [{"start_ms": 0, "title": "x", "end_ms": 9}]}),
        ] {
            assert!(decode::<Open>(bad).is_err());
        }
    }

    #[test]
    fn lyrics_are_a_sheet_with_text() {
        let lyrics: LyricsAnswer =
            decode(json!({"text": "[00:01.00]One", "synced": true})).unwrap();
        assert!(lyrics.synced);

        let plain: LyricsAnswer = decode(json!({"text": "One\nTwo"})).unwrap();
        assert!(!plain.synced);

        // Past the per-string cap, which a sheet routinely is.
        assert!(decode::<LyricsAnswer>(json!({"text": "x".repeat(MAX_STRING * 4)})).is_ok());

        for bad in [
            json!({"text": "  \n"}),
            json!({"text": "x".repeat(MAX_LYRICS + 1)}),
            json!({"text": "x", "source": "y"}),
        ] {
            assert!(decode::<LyricsAnswer>(bad).is_err());
        }
    }

    #[test]
    fn lyrics_candidates_carry_sheets_and_hints() {
        let hit = json!({
            "artist": "A",
            "title": "T",
            "album": "",
            "duration_secs": 241.0,
            "text": "[00:12.00]One",
            "synced": true,
            "confidence": 0.95,
        });
        let good: LyricsCandidates = decode(json!({"candidates": [hit]})).unwrap();
        assert_eq!(good.candidates[0].confidence, Some(0.95));
        assert!(good.candidates[0].synced);

        for bad in [
            json!({"text": "  \n"}),
            json!({"text": "x".repeat(MAX_LYRICS + 1)}),
            json!({"text": "x", "confidence": 1.5}),
            json!({"text": "x", "source": "y"}),
        ] {
            assert!(decode::<LyricsCandidates>(json!({"candidates": [bad]})).is_err());
        }

        let many = vec![json!({"text": "x"}); MAX_CANDIDATES + 1];
        assert!(decode::<LyricsCandidates>(json!({"candidates": many})).is_err());
    }

    #[test]
    fn metadata_candidates_leave_missing_fields_empty() {
        let found: MetadataCandidates = decode(json!({"candidates": [{"title": "T"}]})).unwrap();

        assert_eq!(
            found.candidates[0],
            MetadataHit {
                title: "T".into(),
                ..MetadataHit::default()
            }
        );
        assert_eq!(found.candidates[0].confidence, None);
    }

    #[test]
    fn an_artist_sheet_is_text_only() {
        let sheet: ArtistSheet = decode(json!({
            "bio": "One.\n\nTwo.",
            "url": "https://example.com/artist/1",
            "tags": ["rock"],
            "similar": ["B"],
            "country": "US",
            "formed": 2001,
            "ended": null,
        }))
        .unwrap();
        assert_eq!(sheet.formed, Some(2001));

        for bad in [
            json!({"bio": " "}),
            json!({"bio": "x", "url": "http://example.com/a b"}),
            json!({"bio": "x", "country": "us"}),
            json!({"bio": "x", "country": "USA"}),
            json!({"bio": "x", "image": "https://example.com/a.jpg"}),
        ] {
            assert!(decode::<ArtistSheet>(bad).is_err());
        }
    }

    #[test]
    fn a_link_is_a_web_page_or_nothing() {
        let link: Link = decode(json!({"url": "https://example.com/track/1"})).unwrap();
        assert_eq!(link.url, "https://example.com/track/1");

        for bad in [
            json!({"url": "javascript:alert(1)"}),
            json!({"url": "file:///etc/passwd"}),
            json!({"url": "https://example.com/a b"}),
            json!({"url": "https://example.com", "label": "x"}),
        ] {
            assert!(decode::<Link>(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_action_answers_a_job_or_an_outcome() {
        let job: ActionAnswer = decode(json!({"job": "j1"})).unwrap();
        assert_eq!(job.job.as_deref(), Some("j1"));

        let done: ActionAnswer =
            decode(json!({"message": "Saved", "reveal": "/home/me/Videos/a.mp4"})).unwrap();
        assert_eq!(
            done.outcome().reveal.as_deref(),
            Some("/home/me/Videos/a.mp4")
        );

        assert!(decode::<ActionAnswer>(json!({"job": ""})).is_err());
        assert!(decode::<ActionAnswer>(json!({"then": "x"})).is_err());

        let flagged: ActionAnswer =
            decode(json!({"message": "Added", "flags": {"7": ["favourite"]}})).unwrap();
        assert_eq!(flagged.flags["7"], ["favourite"]);
    }

    #[test]
    fn a_flags_answer_names_each_item_it_knows() {
        let answer: FlagsAnswer =
            decode(json!({"flags": {"t1": ["favourite"], "t2": []}})).unwrap();
        assert_eq!(answer.flags["t1"], ["favourite"]);
        assert!(answer.flags["t2"].is_empty());

        let crowded: Vec<String> = (0..=MAX_FLAGS).map(|i| format!("f{i}")).collect();
        assert!(decode::<FlagsAnswer>(json!({"flags": {"t1": crowded}})).is_err());
        assert!(decode::<FlagsAnswer>(json!({"flags": {}, "more": 1})).is_err());
    }

    #[test]
    fn a_row_may_say_its_flags_or_leave_them_unknown() {
        let page: Page = decode(json!({"entries": [
            {"track": {"key": "t1", "flags": ["favourite"]}},
            {"track": {"key": "t2", "flags": []}},
            {"track": {"key": "t3"}},
            {"node": {"id": "n1", "title": "N", "flags": ["offline"]}},
        ]}))
        .unwrap();

        let flags: Vec<Option<Vec<String>>> = page
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Track(t) => t.flags.clone(),
                Entry::Node(n) => n.flags.clone(),
                Entry::Section(_) => None,
            })
            .collect();
        assert_eq!(
            flags,
            [
                Some(vec!["favourite".to_string()]),
                Some(vec![]),
                None,
                Some(vec!["offline".to_string()]),
            ]
        );

        let crowded: Vec<String> = (0..=MAX_FLAGS).map(|i| format!("f{i}")).collect();
        assert!(
            decode::<Page>(json!({"entries": [{"track": {"key": "t", "flags": crowded}}]}))
                .is_err()
        );
    }

    #[test]
    fn an_outcome_names_only_a_web_link_and_an_absolute_path() {
        let refused = [
            json!({"link": "file:///etc/passwd"}),
            json!({"link": "javascript:alert(1)"}),
            json!({"reveal": "relative/path"}),
            json!({"reveal": "/tmp/a\nb"}),
        ];

        for answer in refused {
            assert!(decode::<ActionAnswer>(answer.clone()).is_err(), "{answer}");
            assert!(decode::<JobState>(answer.clone()).is_err(), "{answer}");
        }
    }

    #[test]
    fn a_job_state_reads_progress_and_its_end() {
        let running: JobState = decode(json!({"done": 3, "total": 10, "text": "a.wav"})).unwrap();
        assert!(!running.finished && running.error.is_none());

        let failed: JobState = decode(json!({"error": "disk full"})).unwrap();
        assert_eq!(failed.error.as_deref(), Some("disk full"));
    }
}
