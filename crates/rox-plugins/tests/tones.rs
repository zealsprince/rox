//! The tones example (`examples/plugins/tones`) against the real host, so the
//! plugin rox publishes for authors to copy keeps working. Skipped with a
//! note when no interpreter resolves, like the echo fixture's tests.

use std::path::{Path, PathBuf};

use rox_plugins::{Host, HostConfig, Options, Stream, hash, loader, manifest, wire};
use serde_json::{Value, json};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins")
}

fn host(tag: &str) -> Option<Host> {
    let dir = examples().join("tones");
    let manifest = manifest::load(&dir).expect("the example's manifest loads");

    if let Err(e) = manifest::entry_for(&manifest, &dir) {
        eprintln!("skipping the tones example tests: {e}");
        return None;
    }

    let data_dir =
        std::env::temp_dir().join(format!("rox-plugins-tones-{tag}-{}", std::process::id()));

    Some(Host::new(HostConfig::new(
        dir,
        manifest,
        json!({}),
        data_dir,
    )))
}

fn call(host: &Host, method: &'static str, params: Value) -> Value {
    host.call(method, params, host.timeouts().sync_first)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

#[test]
fn the_folder_loads_as_a_plugin_and_hashes() {
    let found = loader::scan(
        &examples(),
        &std::env::temp_dir().join(format!("rox-plugins-cache-{}", std::process::id())),
    );
    let tones = found
        .iter()
        .find(|loaded| loaded.id == "tones")
        .expect("the scan finds it");

    assert!(
        tones.runs()
            || tones
                .error
                .as_deref()
                .is_some_and(|e| e.contains("interpreter"))
    );
    assert_eq!(tones.manifest.as_ref().unwrap().api, 1);
    assert!(tones.programs.is_empty());

    let again = hash::folder_hash(&tones.dir).unwrap();
    assert_eq!(tones.hash, again, "the hash is stable");
}

#[test]
fn it_says_hello_and_lists_two_collections() {
    let Some(host) = host("roots") else {
        return;
    };
    let before = hash::folder_hash(&examples().join("tones")).unwrap();

    host.ensure().expect("hello answers");

    let roots = call(
        &host,
        "source.browse",
        json!({"node": null, "cursor": null}),
    );
    let nodes: Vec<(&str, bool)> = roots["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["node"]["id"].as_str().unwrap(),
                e["node"]["collection"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(nodes, vec![("tones", true), ("chords", true)]);
    assert!(roots["cursor"].is_null());

    let found = call(
        &host,
        "source.search",
        json!({"query": "minor", "cursor": null}),
    );
    assert_eq!(found["entries"][0]["track"]["key"], "chord:A minor");

    host.stop("test over");

    let after = hash::folder_hash(&examples().join("tones")).unwrap();
    assert_eq!(
        before, after,
        "running it wrote nothing into its own folder"
    );
}

#[test]
fn a_sync_pages_then_answers_unchanged() {
    let Some(host) = host("sync") else {
        return;
    };

    let mut keys = Vec::new();
    let mut cursor = Value::Null;
    let mut pages = 0;
    let token = loop {
        let page = call(
            &host,
            "source.sync",
            json!({"collection": "tones", "token": null, "cursor": cursor}),
        );
        pages += 1;

        for track in page["tracks"].as_array().unwrap() {
            keys.push(track["key"].as_str().unwrap().to_string());
        }

        cursor = page["cursor"].clone();
        if cursor.is_null() {
            break page["token"].as_str().unwrap().to_string();
        }
        assert!(page["token"].is_null(), "only the last page carries it");
    };

    assert!(pages > 1, "the example pages its syncs");
    assert_eq!(keys.len(), 6);
    assert_eq!(keys[0], "tone:A3");

    let again = call(
        &host,
        "source.sync",
        json!({"collection": "tones", "token": token, "cursor": null}),
    );
    assert_eq!(again["unchanged"], true);

    host.stop("test over");
}

#[test]
fn a_track_reads_whole_as_a_wav_of_its_stated_length() {
    let Some(host) = host("read") else {
        return;
    };

    let listed = call(
        &host,
        "source.browse",
        json!({"node": "chords", "cursor": null}),
    );
    let track = &listed["entries"][0]["track"];
    let duration_ms = track["duration_ms"].as_u64().unwrap();

    let stream = Stream::open(&host, track["key"].as_str().unwrap(), Options::default()).unwrap();
    assert_eq!(stream.hint, "wav");
    assert!(stream.seekable && !stream.live);

    let mut bytes = Vec::new();
    loop {
        let chunk = stream.read_at(bytes.len() as u64, 256 * 1024).unwrap();
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
    }
    assert_eq!(Some(bytes.len() as u64), stream.length);

    drop(stream);
    host.stop("test over");

    let (rate, frames) = decode(bytes);
    assert_eq!(frames, rate as u64 * duration_ms / 1000);
}

#[test]
fn a_chord_names_its_notes_as_chapters_and_lyrics() {
    let Some(host) = host("notes") else {
        return;
    };

    let chord = Stream::open(&host, "chord:A minor", Options::default()).unwrap();
    let starts: Vec<(u64, &str)> = chord
        .chapters
        .iter()
        .map(|chapter| (chapter.start_ms, chapter.title.as_str()))
        .collect();
    assert_eq!(starts, [(0, "A3"), (5000, "C4"), (10_000, "E4")]);

    let tone = Stream::open(&host, "tone:A4", Options::default()).unwrap();
    assert!(tone.chapters.is_empty());

    let sheet: wire::LyricsAnswer = wire::decode(call(
        &host,
        "source.lyrics",
        json!({"key": "chord:A minor"}),
    ))
    .unwrap();
    assert!(sheet.synced);
    assert_eq!(sheet.text, "[00:00.00]A3\n[00:05.00]C4\n[00:10.00]E4");

    let none = call(&host, "source.lyrics", json!({"key": "tone:A4"}));
    assert!(none.is_null(), "a tone has no sheet");

    drop((chord, tone));
    host.stop("test over");
}

#[test]
fn export_runs_as_a_job_and_reveals_its_folder() {
    let Some(host) = host("export") else {
        return;
    };

    let manifest = manifest::load(&examples().join("tones")).unwrap();
    let declared = &manifest.capabilities.source.unwrap().actions[0];
    assert!(declared.offered_on("track") && declared.offered_on("node"));

    // A collection's id stands for every tone in it.
    let started = call(
        &host,
        "source.action",
        json!({"action": "export", "items": ["tones"], "params": {"seconds": 1}}),
    );
    let started: wire::ActionAnswer = wire::decode(started).expect("a job answer");
    let job = started.job.expect("export is a job");

    let state = loop {
        let state: wire::JobState =
            wire::decode(call(&host, "source.job", json!({"job": job}))).unwrap();
        if state.finished || state.error.is_some() {
            break state;
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    host.stop("test over");

    assert_eq!(state.error, None);
    assert_eq!((state.done, state.total), (6, 6));

    let folder = PathBuf::from(state.reveal.expect("the folder to show"));
    assert!(folder.is_absolute());
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 6);
    assert!(
        !folder.starts_with(examples()),
        "the plugin writes outside its own folder"
    );

    std::fs::remove_dir_all(folder.parent().unwrap()).ok();
}

/// Probes and decodes the bytes, answering the sample rate and the frames
/// actually decoded.
fn decode(bytes: Vec<u8>) -> (u32, u64) {
    let source = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("wav");

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .expect("the WAV probes");

    let track = format.default_track(TrackType::Audio).unwrap();
    let id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .unwrap()
        .audio()
        .unwrap()
        .clone();
    let rate = params.sample_rate.unwrap();

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .expect("a decoder for it");

    let mut frames = 0u64;
    while let Some(packet) = format.next_packet().expect("packets read") {
        if packet.track_id == id {
            frames += decoder.decode(&packet).expect("packets decode").frames() as u64;
        }
    }

    (rate, frames)
}
