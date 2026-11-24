//! The control socket's document verbs: the imports and installs, and the
//! exports that stand in for a save prompt. Each ends in the function the
//! window runs once its picker has a path, so a file a script hands over is
//! stored or listed and nothing more, and a file it asks for is written
//! where it says and nowhere else. None applies a look, places an object or
//! switches a plugin on (ADR 35).

use std::path::{Component, Path, PathBuf};

use gpui::App;
use serde_json::{Value, json};

use rox_core::settings::{AssetKind, WorkspaceMeta, assets};
use rox_ipc::{Request, RpcError};
use rox_services::plugins as host;

/// Stored under a deduped name and offered through the apply confirm in the
/// front window, the same as a double-click. Answers the stored name.
pub fn import_workspace(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let path = match path_param(&params) {
        Ok(path) => path,
        Err(e) => return responder.respond(Err(e)),
    };

    cx.spawn(async move |cx| {
        let imported = cx
            .background_executor()
            .spawn(async move { crate::workspaces::import_file(&path) })
            .await;

        let result = match imported {
            Ok(bundle) => {
                let reply = json!({ "name": bundle.name });
                cx.update(|cx| {
                    crate::workspace::with_front_workspace(cx, move |ws, _, cx| {
                        ws.offer_imported(&bundle, cx);
                    });
                })
                .ok();
                Ok(reply)
            }
            Err(e) => Err(RpcError::app(e)),
        };
        responder.respond(result);
    })
    .detach();
}

/// Listed, never switched on. Answers the plugin's id.
pub fn install_plugin(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let path = match path_param(&params) {
        Ok(path) => path,
        Err(e) => return responder.respond(Err(e)),
    };

    cx.spawn(async move |cx| {
        // The hash and the copy scale with the archive, so neither runs on
        // the UI thread.
        let installed = cx
            .background_executor()
            .spawn(async move { host::install_file(&path) })
            .await;

        let result = match installed {
            Ok(id) => {
                cx.update(host::rescan).ok();
                Ok(json!({ "id": id }))
            }
            Err(e) => Err(RpcError::app(e)),
        };
        responder.respond(result);
    })
    .detach();
}

/// A file into the live workspace's assets, through the checks the
/// Workspace page's Add runs. Answers the asset's id.
pub fn import_asset(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let path = match file_param(&params, "path") {
        Ok(path) => path,
        Err(e) => return responder.respond(Err(e)),
    };

    let Some(file) = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
    else {
        return responder.respond(Err(RpcError::invalid_params("the path names no file")));
    };
    if let Some(refusal) = crate::asset_import::refusal(&path, &file) {
        return responder.respond(Err(RpcError::app(format!("{file}: {}", refusal.english()))));
    }

    cx.spawn(async move |cx| {
        let max_bytes = crate::asset_import::max_bytes(&file);
        let staged = cx
            .background_executor()
            .spawn(async move { assets::stage(&path, max_bytes) })
            .await;

        // The table write belongs on the UI thread, where an apply swaps it.
        let result = cx
            .update(|cx| staged.and_then(|staged| crate::asset_import::commit(staged, cx)))
            .map_err(RpcError::app)
            .and_then(|committed| committed.map_err(RpcError::app))
            .map(|asset| {
                json!({
                    "id": asset.id,
                    "file": asset.file,
                    "kind": match asset.kind {
                        AssetKind::Image => "image",
                        AssetKind::Font => "font",
                    },
                    "bytes": asset.bytes,
                })
            });
        responder.respond(result);
    })
    .detach();
}

/// Saves the live look under `name`, as the Workspace page's Save does. A
/// name already saved is refused: the page asks before it overwrites one,
/// and a script has no one to ask.
pub fn save_workspace(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let name = match name_param(&params) {
        Ok(name) => name,
        Err(e) => return responder.respond(Err(e)),
    };
    if crate::workspaces::path_for(&name).exists() {
        return responder.respond(Err(RpcError::app(format!(
            "a workspace named {name:?} is already saved"
        ))));
    }

    let store = move || {
        let bundle = crate::workspaces::snapshot(&name, &rox_core::settings::Settings::load());
        match crate::workspaces::store(&bundle) {
            true => Ok(json!({ "name": bundle.name })),
            false => Err(RpcError::app(
                "the workspace couldn't be saved; the log says why",
            )),
        }
    };

    // Panel config like the library's columns reaches disk on the next dump,
    // so the front window persists first, as the page flushes before a save.
    if rox_panel_api::windows::front_workspace(cx).is_none() {
        return responder.respond(store());
    }
    crate::workspace::with_front_workspace(cx, move |ws, window, cx| {
        ws.persist(window, cx);
        responder.respond(store());
    });
}

/// A saved or shipped workspace written out as its `.roxw`, the Workspace
/// page's Export without the save prompt.
pub fn export_workspace(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let parsed = name_param(&params).and_then(|name| Ok((name, out_param(&params, "path")?)));
    let (name, out) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => return responder.respond(Err(e)),
    };
    let Some((_, origin)) = crate::workspaces::resolve_with_origin(&name) else {
        return responder.respond(Err(RpcError::app(format!("no workspace named {name:?}"))));
    };

    cx.spawn(async move |cx| {
        let target = out.clone();
        let written = cx
            .background_executor()
            .spawn(async move { crate::workspaces::export_to(&name, &origin, &target) })
            .await;

        responder.respond(
            written
                .map(|bytes| json!({ "path": out, "bytes": bytes }))
                .map_err(RpcError::app),
        );
    })
    .detach();
}

/// A canvas item lifted out as a `.roxo`, the inspector's Export Object
/// card and its save prompt in one call. The item's id names the canvas: the
/// first canvas in each of the window's docks that holds it.
pub fn export_object(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let item = params
        .get("item")
        .and_then(Value::as_str)
        .filter(|item| !item.is_empty())
        .ok_or_else(|| {
            RpcError::invalid_params("takes {\"item\": a canvas item's id, \"path\"}")
        })?;
    let out = out_param(params, "path")?;
    let text = |field: &str| {
        params
            .get(field)
            .and_then(Value::as_str)
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    };

    let window = params.get("window").and_then(Value::as_u64);
    let workspace = super::ipc::workspace_for(window, cx)?;
    let canvas = workspace
        .read(cx)
        .docks(cx)
        .iter()
        .filter_map(|dock| {
            dock.read(cx)
                .panel_named(crate::panels::canvas::PANEL_NAME, cx)
        })
        .filter_map(|panel| {
            panel
                .view()
                .downcast::<crate::panels::canvas::CanvasPanel>()
                .ok()
        })
        .find(|canvas| canvas.read(cx).saved(cx).canvas.find(item).is_some())
        .ok_or_else(|| RpcError::app(format!("no canvas in that window holds an item {item:?}")))?;

    // The card's name field starts as the item's name, or its id.
    let name = match text("name") {
        name if !name.is_empty() => name,
        _ => {
            let saved = canvas.read(cx).saved(cx);
            saved
                .canvas
                .find(item)
                .map(|found| match found.name.trim() {
                    "" => found.id.clone(),
                    name => name.to_string(),
                })
                .unwrap_or_else(|| item.to_string())
        }
    };
    let card = crate::objects::Card {
        name: name.clone(),
        meta: WorkspaceMeta {
            author: text("author"),
            description: text("description"),
            license: text("license"),
            ..WorkspaceMeta::default()
        },
    };

    crate::objects::export(canvas.read(cx), item, card, &out, cx).map_err(RpcError::app)?;

    Ok(json!({ "path": out, "name": name }))
}

/// An installed plugin's folder packed as a `.roxp`, the Plugins page's
/// Pack without the save prompt. Neither the folder nor its switch changes.
pub fn pack_plugin(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();
    let id = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params("takes {\"id\": a plugin id, \"path\"}"));
    let parsed = id.and_then(|id| Ok((id, out_param(&params, "path")?)));
    let (id, out) = match parsed {
        Ok(parsed) => parsed,
        Err(e) => return responder.respond(Err(e)),
    };

    cx.spawn(async move |cx| {
        // Packing hashes and compresses the whole folder.
        let target = out.clone();
        let packed = cx
            .background_executor()
            .spawn(async move { host::pack(&id, &target) })
            .await;

        responder.respond(
            packed
                .map(|()| json!({ "path": out }))
                .map_err(RpcError::app),
        );
    })
    .detach();
}

fn name_param(params: &Value) -> Result<String, RpcError> {
    params
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RpcError::invalid_params("takes {\"name\": a workspace name}"))
}

/// rox runs in its own working directory, so a relative path would name a
/// different file from the one the caller meant.
pub(super) fn path_param(params: &Value) -> Result<PathBuf, RpcError> {
    absolute_param(params, "path")
}

fn absolute_param(params: &Value, key: &str) -> Result<PathBuf, RpcError> {
    let path = params
        .get(key)
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            RpcError::invalid_params(format!("takes {{\"{key}\": an absolute path}}"))
        })?;

    if !path.is_absolute() {
        return Err(RpcError::invalid_params(format!(
            "{} isn't an absolute path",
            path.display()
        )));
    }
    Ok(path)
}

/// A file to read in: there, and a file.
pub(super) fn file_param(params: &Value, key: &str) -> Result<PathBuf, RpcError> {
    let path = absolute_param(params, key)?;

    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(path),
        Ok(_) => Err(RpcError::app(format!("{} isn't a file", path.display()))),
        Err(e) => Err(RpcError::app(format!("{}: {e}", path.display()))),
    }
}

/// A folder to read: there, a folder, and spelled without `.` or `..`, so
/// comparing it against the folders already listed means what it says.
pub(super) fn dir_param(params: &Value, key: &str) -> Result<PathBuf, RpcError> {
    let path = absolute_param(params, key)?;

    if path
        .components()
        .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(RpcError::invalid_params(format!(
            "{} holds . or ..; give the folder's plain path",
            path.display()
        )));
    }
    match std::fs::metadata(&path) {
        Ok(meta) if meta.is_dir() => Ok(path),
        Ok(_) => Err(RpcError::app(format!("{} isn't a folder", path.display()))),
        Err(e) => Err(RpcError::app(format!("{}: {e}", path.display()))),
    }
}

/// Where an export goes: a new file in a folder that exists. Nothing is
/// written over. The window's save prompt asks before it replaces a file,
/// and a socket call has no one to ask.
pub(super) fn out_param(params: &Value, key: &str) -> Result<PathBuf, RpcError> {
    let path = absolute_param(params, key)?;

    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        return Err(RpcError::app(match meta.is_dir() {
            true => format!("{} is a folder; name the file to write", path.display()),
            false => format!("{} already exists; rox won't write over it", path.display()),
        }));
    }
    match path.parent().map(Path::is_dir) {
        Some(true) => Ok(path),
        _ => Err(RpcError::app(format!(
            "{} has no folder to go in",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of its own under the temp dir, holding one file.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-ipc-docs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("look.roxw"), b"zip").unwrap();
        dir
    }

    fn code(result: Result<PathBuf, RpcError>) -> Option<i64> {
        result.err().map(|e| e.code)
    }

    #[test]
    fn a_file_to_read_has_to_be_an_absolute_file_that_exists() {
        let dir = scratch("file");
        let at = |path: &Path| json!({ "path": path });

        assert_eq!(
            code(file_param(&json!({ "path": "look.roxw" }), "path")),
            Some(-32602)
        );
        assert_eq!(
            code(file_param(&at(&dir.join("gone.roxw")), "path")),
            Some(-32000)
        );
        assert_eq!(code(file_param(&at(&dir), "path")), Some(-32000));
        assert_eq!(
            file_param(&at(&dir.join("look.roxw")), "path").ok(),
            Some(dir.join("look.roxw"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_has_to_be_an_absolute_plain_folder_that_exists() {
        let dir = scratch("dir");
        let at = |path: &Path| json!({ "path": path });

        assert_eq!(
            code(dir_param(&json!({ "path": "music" }), "path")),
            Some(-32602)
        );
        assert_eq!(
            code(dir_param(&at(&dir.join("gone")), "path")),
            Some(-32000)
        );
        assert_eq!(
            code(dir_param(&at(&dir.join("look.roxw")), "path")),
            Some(-32000)
        );
        assert_eq!(code(dir_param(&at(&dir.join("..")), "path")), Some(-32602));
        assert_eq!(dir_param(&at(&dir), "path").ok(), Some(dir.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_export_goes_to_a_new_file_in_a_folder_that_exists() {
        let dir = scratch("out");
        let at = |path: &Path| json!({ "path": path });

        assert_eq!(
            code(out_param(&json!({ "path": "out.roxw" }), "path")),
            Some(-32602)
        );
        assert_eq!(code(out_param(&at(&dir), "path")), Some(-32000));
        assert_eq!(
            code(out_param(&at(&dir.join("look.roxw")), "path")),
            Some(-32000)
        );
        assert_eq!(
            code(out_param(&at(&dir.join("gone").join("out.roxw")), "path")),
            Some(-32000)
        );
        assert_eq!(
            out_param(&at(&dir.join("out.roxw")), "path").ok(),
            Some(dir.join("out.roxw"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_is_trimmed_and_never_empty() {
        assert!(name_param(&json!({})).is_err());
        assert!(name_param(&json!({ "name": "  " })).is_err());
        assert_eq!(
            name_param(&json!({ "name": " Night " })).ok().as_deref(),
            Some("Night")
        );
    }

    #[test]
    fn a_path_has_to_be_there_and_absolute() {
        let code = |params: Value| path_param(&params).err().map(|e| e.code);

        assert_eq!(code(json!({})), Some(-32602));
        assert_eq!(code(json!({ "path": 3 })), Some(-32602));
        assert_eq!(code(json!({ "path": "" })), Some(-32602));
        assert_eq!(code(json!({ "path": "Look.roxw" })), Some(-32602));

        #[cfg(unix)]
        assert_eq!(
            path_param(&json!({ "path": "/m/Look.roxw" })).ok(),
            Some(PathBuf::from("/m/Look.roxw"))
        );
    }
}
