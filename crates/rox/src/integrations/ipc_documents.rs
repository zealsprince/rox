//! The control socket's document verbs: `workspace.import` and
//! `plugins.install`. They end in the same import and install the window
//! runs, so a file a script hands over is stored or listed and nothing
//! more. Neither applies a look or switches a plugin on.

use std::path::PathBuf;

use gpui::App;
use serde_json::{Value, json};

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

/// rox runs in its own working directory, so a relative path would name a
/// different file from the one the caller meant.
fn path_param(params: &Value) -> Result<PathBuf, RpcError> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| RpcError::invalid_params("takes {\"path\": an absolute file path}"))?;

    if !path.is_absolute() {
        return Err(RpcError::invalid_params(format!(
            "{} isn't an absolute path",
            path.display()
        )));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

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
