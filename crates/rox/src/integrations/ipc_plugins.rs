//! The control socket's `plugins.*` methods (ADR 22's plugins amendment):
//! listing what a plugin offers, browsing and searching it, and running the
//! actions it declares, through the same calls the source browser and the
//! action menus make.
//!
//! Every call here is one rox makes into the plugin, so ADR 30's line holds: a
//! plugin can't tell a socket client was behind it, and gets no way to call
//! back. Everything a plugin answers is its own text, passed through as data.

use std::path::PathBuf;

use gpui::{AnyWindowHandle, App, Task};
use serde_json::{Map, Value, json};

use rox_ipc::{Request, Responder, RpcError};
use rox_library::cue::{PLUGIN_PREFIX, TrackKey, source_id};
use rox_panel_api::panel::AppState;
use rox_services::catalog::Library;
use rox_services::plugin_actions::{self, ActionDecl, Started};
use rox_services::plugins::{self, Entry, FieldValue, Page, Unavailable};

/// Every plugin that answers, with the actions it declares.
pub fn list() -> Value {
    let plugins: Vec<Value> = plugins::searchable_sources()
        .into_iter()
        .map(|(source, label)| {
            let actions: Vec<Value> = plugin_actions::actions(&source)
                .into_iter()
                .map(|action| {
                    json!({
                        "id": action.id,
                        "label": action.label,
                        "on": action.on,
                        "params": action.params,
                        "when": action.when,
                    })
                })
                .collect();

            json!({ "source": source, "label": label, "actions": actions })
        })
        .collect();

    json!({ "enabled": plugins::allowed(), "plugins": plugins })
}

pub fn browse(state: &AppState, request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let source = match source(&params) {
        Ok(source) => source,
        Err(e) => return responder.respond(Err(e)),
    };

    let task = plugins::browse(
        &source,
        text(&params, "node"),
        text(&params, "view"),
        text(&params, "cursor"),
        cx,
    );
    answer_page(state, source, task, responder, cx);
}

pub fn search(state: &AppState, request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let source = match source(&params) {
        Ok(source) => source,
        Err(e) => return responder.respond(Err(e)),
    };
    let Some(query) = text(&params, "query") else {
        return responder.respond(Err(RpcError::invalid_params(
            "search takes {\"source\", \"query\", \"view\"?, \"cursor\"?}",
        )));
    };

    let task = plugins::search(
        &source,
        query,
        text(&params, "view"),
        text(&params, "cursor"),
        cx,
    );
    answer_page(state, source, task, responder, cx);
}

/// An action that finishes at once answers with its outcome; one that starts
/// a job answers with the job's number, which `tasks.status` reports under
/// `plugin_jobs` and `tasks.stop` takes as `job`. Either way the user gets the
/// toast a menu's action would give, titled as a client's.
pub fn action(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let (source, action, items, args) = match prepare(&params) {
        Ok(prepared) => prepared,
        Err(e) => return responder.respond(Err(e)),
    };

    let task = plugin_actions::run(&source, &action, items, args, cx);
    let label = action.label;

    cx.spawn(async move |cx| {
        let started = task.await;

        let result = cx.update(|cx| match started {
            Ok(Started::Done(outcome)) => {
                let reply = json!({
                    "done": true,
                    "message": outcome.message,
                    "link": outcome.link,
                    "reveal": outcome.reveal,
                });

                if let Some(origin) = origin(cx) {
                    rox_panel_api::plugin_actions::client_ran(&label, outcome, origin, cx);
                }
                Ok(reply)
            }

            Ok(Started::Job(job)) => {
                let reply = json!({ "done": false, "job": job.serial, "message": job.text() });

                // Unwatched, a job never polls and never leaves the Tasks window.
                match origin(cx) {
                    Some(origin) => {
                        rox_panel_api::plugin_actions::watch_client_job(job, label, origin, cx)
                    }
                    None => plugin_actions::watch(job, cx).detach(),
                }
                Ok(reply)
            }

            Err(e) => {
                log::warn!("{source}: client action {label}: {e}");

                if let Some(origin) = origin(cx) {
                    rox_panel_api::plugin_actions::client_failed(&label, e.clone(), origin, cx);
                }
                Err(RpcError::app(e))
            }
        });

        match result {
            Ok(result) => responder.respond(result),
            Err(_) => responder.respond(Err(RpcError::app("rox is shutting down"))),
        }
    })
    .detach();
}

/// The jobs plugin actions are running, for `tasks.status`.
pub fn jobs() -> Value {
    plugin_actions::jobs()
        .iter()
        .map(|job| {
            json!({
                "job": job.serial,
                "plugin": job.plugin,
                "action": job.label,
                "done": job.done(),
                "total": job.total(),
                "text": job.text(),
                "stopping": job.stopping(),
            })
        })
        .collect()
}

/// The Tasks window's Stop: the plugin hears `source.cancel` on the next poll.
pub fn stop_job(serial: u64) -> Result<Value, RpcError> {
    let job = plugin_actions::job(serial)
        .ok_or_else(|| RpcError::app(format!("no plugin job {serial} is running")))?;

    job.stop();
    Ok(json!({ "stopping": true }))
}

/// Toasts land in the window the user is in, or any open one.
fn origin(cx: &App) -> Option<AnyWindowHandle> {
    cx.active_window().or_else(|| cx.windows().first().copied())
}

fn text(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Takes the source as `plugins.list` names it, or the bare plugin id.
fn source(params: &Value) -> Result<String, RpcError> {
    let named = text(params, "source").ok_or_else(|| {
        RpcError::invalid_params("takes {\"source\"}, a plugin as plugins.list names it")
    })?;
    let source = match named.starts_with(PLUGIN_PREFIX) {
        true => named,
        false => format!("{PLUGIN_PREFIX}{named}"),
    };

    if plugins::searchable_sources()
        .iter()
        .any(|(answers, _)| *answers == source)
    {
        return Ok(source);
    }

    let reason = match plugins::unavailable(&source) {
        Some(why) => unavailable(&source, why),
        None => format!("{source} lists nothing: it declares no source"),
    };
    Err(RpcError::app(reason))
}

fn unavailable(source: &str, why: Unavailable) -> String {
    match why {
        Unavailable::PluginsOff => {
            "plugins are switched off in rox; \"Enable Plugins\" on Settings > Plugins turns \
             them on"
                .into()
        }
        Unavailable::SwitchedOff => format!("{source} is switched off on Settings > Plugins"),
        Unavailable::Changed => format!(
            "{source} changed on disk since it was switched on, and waits on Settings > \
             Plugins to be approved again"
        ),
        Unavailable::Missing => format!("there's no plugin {source} in the plugins folder"),
        Unavailable::Failed => format!("{source} doesn't load; Settings > Plugins says why"),
        Unavailable::Stopped => format!("{source} was stopped after crashing repeatedly"),
    }
}

fn answer_page(
    state: &AppState,
    source: String,
    task: Task<Result<Page, String>>,
    responder: Responder,
    cx: &mut App,
) {
    let library = state.library.clone();

    cx.spawn(async move |cx| {
        let page = match task.await {
            Ok(page) => page,
            Err(e) => return responder.respond(Err(RpcError::app(e))),
        };

        let result = cx.update(|cx| page_json(&source, page, library.read(cx)));
        match result {
            Ok(result) => responder.respond(Ok(result)),
            Err(_) => responder.respond(Err(RpcError::app("rox is shutting down"))),
        }
    })
    .detach();
}

/// A track's `key` is what an action takes as an item. `library_key` is set
/// when the library holds the track, and is what `queue.add` takes.
fn page_json(source: &str, page: Page, library: &Library) -> Value {
    let flags = |id: &str| page.flags.get(id).cloned().unwrap_or_default();
    let values = |at: usize| -> Map<String, Value> {
        page.values
            .get(at)
            .into_iter()
            .flatten()
            .map(|(id, value)| {
                let value = match value {
                    FieldValue::Number(n) => json!(n),
                    FieldValue::Text(text) => json!(text),
                };
                (id.clone(), value)
            })
            .collect()
    };

    let entries: Vec<Value> = page
        .entries
        .iter()
        .enumerate()
        .map(|(at, entry)| match entry {
            Entry::Node {
                id,
                title,
                subtitle,
                collection,
                kind,
                ..
            } => json!({
                "type": "node",
                "id": id,
                "title": title,
                "subtitle": subtitle,
                "collection": collection,
                "kind": kind.map(|kind| kind.name()),
                "flags": flags(id),
                "values": values(at),
            }),

            Entry::Track(track) => {
                let key = TrackKey {
                    source: source_id(source),
                    path: PathBuf::from(&track.key),
                    sub: 0,
                };
                let library_key = library.id_for_key(&key).map(|_| key.to_fragment());

                json!({
                    "type": "track",
                    "key": track.key,
                    "library_key": library_key,
                    "title": track.title,
                    "artist": track.artist,
                    "album_artist": track.album_artist,
                    "album": track.album,
                    "genre": track.genre,
                    "year": track.year,
                    "disc_no": track.disc_no,
                    "track_no": track.track_no,
                    "duration_ms": track.duration_ms,
                    "live": track.live,
                    "flags": flags(&track.key),
                    "values": values(at),
                })
            }

            Entry::Section { title, .. } => json!({ "type": "section", "title": title }),
        })
        .collect();

    json!({
        "entries": entries,
        "cursor": page.cursor,
        "notice": page.notice.map(|notice| notice.text),
        "fields": page
            .fields
            .iter()
            .map(|field| json!({ "id": field.id, "label": field.label }))
            .collect::<Vec<_>>(),
        "views": page
            .views
            .iter()
            .map(|view| json!({ "id": view.id, "label": view.label }))
            .collect::<Vec<_>>(),
        "view": page.view,
    })
}

fn prepare(params: &Value) -> Result<(String, ActionDecl, Vec<String>, Value), RpcError> {
    let source = source(params)?;
    let id = text(params, "action").ok_or_else(|| {
        RpcError::invalid_params("action takes {\"source\", \"action\", \"items\"?, \"params\"?}")
    })?;
    let action = plugin_actions::actions(&source)
        .into_iter()
        .find(|action| action.id == id)
        .ok_or_else(|| {
            RpcError::app(format!(
                "{source} declares no action {id}; plugins.list names the ones it does"
            ))
        })?;

    let items: Vec<String> = match params.get("items") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or_else(|| {
                RpcError::invalid_params("items are strings: tracks' keys or nodes' ids")
            })?,
        Some(_) => {
            return Err(RpcError::invalid_params(
                "items is a list of tracks' keys or nodes' ids",
            ));
        }
    };

    offered(&source, &action, &items)?;
    let args = action_params(&action, params.get("params"))?;

    Ok((source, action, items, args))
}

/// The menus' rule: an action shows where it's declared, and on a set of
/// items when any of them can take it. An item whose flags no listing has
/// reported yet can take anything, and the plugin has the last word.
fn offered(source: &str, action: &ActionDecl, items: &[String]) -> Result<(), RpcError> {
    let declared = match items.is_empty() {
        true => action.offered_on("source"),
        false => action.offered_on("track") || action.offered_on("node"),
    };
    if !declared {
        return Err(RpcError::app(format!(
            "{} runs on {}, so it takes {}",
            action.id,
            action.on.join(" or "),
            match items.is_empty() {
                true => "items",
                false => "no items",
            }
        )));
    }

    let known = plugin_actions::known_flags(source);
    let fits = items.is_empty()
        || items
            .iter()
            .any(|item| action.applies_to(known.get(item).map(Vec::as_slice)));
    if !fits {
        return Err(RpcError::app(format!(
            "{} needs items flagged {:?}, and none of these are",
            action.id, action.when
        )));
    }

    Ok(())
}

/// Held to what the action's form would let through: declared keys only,
/// required ones present, each value the type or one of the choices the
/// schema names.
fn action_params(action: &ActionDecl, given: Option<&Value>) -> Result<Value, RpcError> {
    let given = match given {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(given)) => given.clone(),
        Some(_) => return Err(RpcError::invalid_params("params is an object")),
    };
    let fields: Map<String, Value> = action.param_fields().into_iter().collect();

    for (key, value) in &given {
        let Some(schema) = fields.get(key) else {
            return Err(RpcError::invalid_params(format!(
                "{} takes no param {key}",
                action.id
            )));
        };
        if !fits(schema, value) {
            return Err(RpcError::invalid_params(format!(
                "{key} doesn't fit its schema: {schema}"
            )));
        }
    }

    let missing: Vec<&str> = action.params["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|key| !given.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Err(RpcError::invalid_params(format!(
            "{} needs {}",
            action.id,
            missing.join(", ")
        )));
    }

    Ok(Value::Object(given))
}

fn fits(schema: &Value, value: &Value) -> bool {
    if let Some(options) = schema["enum"].as_array() {
        return options.contains(value);
    }

    match schema["type"].as_str() {
        Some("boolean") => value.is_boolean(),
        Some("integer") => value.is_i64() || value.is_u64(),
        Some("number") => value.is_number(),
        Some("string") => value.is_string(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(on: &[&str], when: &str, params: Value) -> ActionDecl {
        serde_json::from_value(json!({
            "id": "save",
            "label": "Save",
            "on": on,
            "when": when,
            "params": params,
        }))
        .unwrap()
    }

    fn quality() -> Value {
        json!({
            "type": "object",
            "properties": {
                "quality": { "type": "string", "enum": ["low", "high"] },
                "count": { "type": "integer" },
            },
            "required": ["quality"],
        })
    }

    #[test]
    fn params_hold_to_the_schema() {
        let action = action(&["track"], "", quality());

        assert!(action_params(&action, Some(&json!({ "quality": "high" }))).is_ok());
        assert!(action_params(&action, Some(&json!({ "quality": "best" }))).is_err());
        assert!(action_params(&action, Some(&json!({ "quality": "low", "count": 1.5 }))).is_err());
        assert!(action_params(&action, Some(&json!({ "count": 2 }))).is_err());
        assert!(action_params(&action, Some(&json!({ "quality": "low", "to": "/" }))).is_err());
    }

    #[test]
    fn an_action_without_params_takes_none() {
        let action = action(&["source"], "", Value::Null);

        assert_eq!(action_params(&action, None).unwrap(), json!({}));
        assert!(action_params(&action, Some(&json!({ "x": 1 }))).is_err());
    }

    #[test]
    fn items_follow_where_the_action_is_offered() {
        let source = "plugin:ipc-offer-test";
        let on_tracks = action(&["track"], "", Value::Null);
        let on_source = action(&["source"], "", Value::Null);

        assert!(offered(source, &on_tracks, &["t1".into()]).is_ok());
        assert!(offered(source, &on_tracks, &[]).is_err());
        assert!(offered(source, &on_source, &[]).is_ok());
        assert!(offered(source, &on_source, &["t1".into()]).is_err());
    }

    #[test]
    fn known_flags_refuse_an_action_no_item_can_take() {
        let source = "plugin:ipc-flags-test";
        let favourite = action(&["track"], "!favourite", Value::Null);
        plugin_actions::note(
            source,
            &[("t1".to_string(), vec!["favourite".to_string()])].into(),
        );

        assert!(offered(source, &favourite, &["t1".into()]).is_err());
        assert!(offered(source, &favourite, &["t1".into(), "t2".into()]).is_ok());
    }
}
