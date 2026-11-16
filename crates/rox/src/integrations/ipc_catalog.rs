//! `debug.catalog`, the debug scope's way to drive the catalog client
//! (ADR 22) until a window does. Compiled into debug builds only, with the
//! origin override it's paired with. An install it starts ends in the same
//! confirm any other does: the verb approves, applies and places nothing.

use gpui::App;
use serde_json::{Value, json};

use rox_ipc::{Request, RpcError};

/// One of `{"refresh": true}`, `{"install": "<id>"}`, `{"updates": true}`
/// or `{"cancel": true}`. Answers once the refresh or the install is done.
pub fn request(request: Request, cx: &mut App) {
    let (_, params, responder) = request.into_parts();

    if params.get("refresh").and_then(Value::as_bool) == Some(true) {
        let refreshing = crate::catalog::refresh(cx);
        cx.spawn(async move |_| {
            let answer = refreshing.await.map(|index| {
                json!({
                    "packages": index.packages.iter().map(|summary| json!({
                        "id": summary.id,
                        "kind": summary.kind,
                        "sha256": summary.latest.sha256,
                    })).collect::<Vec<_>>(),
                    "dropped": index.dropped,
                })
            });
            responder.respond(answer.map_err(RpcError::app));
        })
        .detach();
        return;
    }

    if let Some(id) = params.get("install").and_then(Value::as_str) {
        let installing = crate::catalog::install(id, cx);
        cx.spawn(async move |_| {
            let answer = installing
                .await
                .and_then(|installed| serde_json::to_value(installed).map_err(|e| e.to_string()));
            responder.respond(answer.map_err(RpcError::app));
        })
        .detach();
        return;
    }

    if params.get("updates").and_then(Value::as_bool) == Some(true) {
        cx.background_executor()
            .spawn(async move {
                let updates: Vec<Value> = crate::catalog::updates()
                    .iter()
                    .map(|summary| json!({ "id": summary.id, "sha256": summary.latest.sha256 }))
                    .collect();
                responder.respond(Ok(json!({ "updates": updates })));
            })
            .detach();
        return;
    }

    if params.get("cancel").and_then(Value::as_bool) == Some(true) {
        let id = crate::catalog::running(cx).map(|(id, _)| id);
        crate::catalog::cancel(cx);
        return responder.respond(Ok(json!({ "cancelled": id })));
    }

    responder.respond(Err(RpcError::invalid_params(
        "takes {\"refresh\": true}, {\"install\": \"<id>\"}, {\"updates\": true} or {\"cancel\": true}",
    )));
}
