//! The debug scope's drive half (ADR 22): synthetic input and action
//! dispatch over the control socket, so a script or an agent can work the
//! UI of a live rox without OS input tools. Everything lands in gpui's own
//! event pipeline - `dispatch_event`, `dispatch_keystroke`, and the action
//! registry - which is what makes it work the same on every platform and on
//! any compositor, including ones with no injection surface at all.
//!
//! Methods: `debug.windows` lists what's open with the ids the rest take,
//! `debug.actions` and `debug.action` cover the command surface by name,
//! `debug.key` and `debug.type` the keyboard, `debug.click`, `debug.hover`,
//! and `debug.scroll` the mouse at window-local logical coordinates.
//! Coordinates and state come from `debug.windows` and `debug.panels`;
//! pixels stay a screenshot job.

use gpui::{
    point, px, App, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, TouchPhase,
};
use serde_json::{json, Value};

use rox_ipc::RpcError;

/// Route one drive method, or `None` when the method isn't ours.
pub fn route(method: &str, params: &Value, cx: &mut App) -> Option<Result<Value, RpcError>> {
    Some(match method {
        "debug.windows" => windows(cx),
        "debug.actions" => actions(params, cx),
        "debug.action" => action(params, cx),
        "debug.key" => key(params, cx),
        "debug.type" => type_text(params, cx),
        "debug.click" => click(params, cx),
        "debug.hover" => hover(params, cx),
        "debug.scroll" => scroll(params, cx),
        _ => return None,
    })
}

/// Every open window: the id the other methods target, the title rox last
/// set on it, its size in the logical pixels the input methods speak, and
/// whether the platform calls it active.
fn windows(cx: &mut App) -> Result<Value, RpcError> {
    let mut rows = Vec::new();
    for handle in cx.windows() {
        let id = handle.window_id().as_u64();
        let row = handle.update(cx, |_, window, _| {
            let size = window.viewport_size();
            json!({
                "id": id,
                "title": rox_panel_api::windows::window_title(id),
                "width": f64::from(size.width),
                "height": f64::from(size.height),
                "scale": window.scale_factor(),
                "active": window.is_window_active(),
            })
        });
        if let Ok(row) = row {
            rows.push(row);
        }
    }
    Ok(json!({ "windows": rows }))
}

/// The window a drive method lands in: the one named by `window`, else the
/// active window, else the first open one, so the plain case needs no id.
fn target(params: &Value, cx: &mut App) -> Result<gpui::AnyWindowHandle, RpcError> {
    if let Some(id) = params.get("window").and_then(Value::as_u64) {
        return cx
            .windows()
            .into_iter()
            .find(|handle| handle.window_id().as_u64() == id)
            .ok_or_else(|| RpcError::app(format!("no window {id} (see debug.windows)")));
    }
    cx.active_window()
        .or_else(|| cx.windows().into_iter().next())
        .ok_or_else(|| RpcError::app("no window open"))
}

/// Registered action names, optionally narrowed to a substring. What
/// `debug.action` will accept; registration doesn't promise a binding in
/// the current focus chain, only that the name builds.
fn actions(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let filter = params
        .get("filter")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    let mut names: Vec<&str> = cx
        .all_action_names()
        .iter()
        .copied()
        .filter(|name| name.to_lowercase().contains(&filter))
        .collect();
    names.sort_unstable();
    Ok(json!({ "actions": names }))
}

/// Build an action by name and dispatch it down the target window's focus
/// chain, exactly as a keybinding would. `data` feeds actions that carry
/// payload the way a keymap entry does.
fn action(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
        RpcError::invalid_params("action takes {\"name\", \"data\"?, \"window\"?}")
    })?;
    let data = params.get("data").filter(|data| !data.is_null()).cloned();
    let action = cx
        .build_action(name, data)
        .map_err(|err| RpcError::app(format!("can't build {name}: {err}")))?;
    let window = target(params, cx)?;
    window
        .update(cx, |_, window, cx| window.dispatch_action(action, cx))
        .map_err(RpcError::app)?;
    Ok(Value::Null)
}

/// Send keystrokes, gpui keymap syntax, space separated: "ctrl-comma",
/// "escape", "cmd-shift-p enter". Answers per stroke with whether anything
/// handled it, so a probe can tell a live binding from a dead one.
fn key(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let keys = params
        .get("keys")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("key takes {\"keys\": \"..\", \"window\"?}"))?;
    let strokes = keys
        .split_whitespace()
        .map(Keystroke::parse)
        .collect::<Result<Vec<_>, _>>()
        .map_err(RpcError::invalid_params)?;
    if strokes.is_empty() {
        return Err(RpcError::invalid_params("key takes at least one keystroke"));
    }
    let window = target(params, cx)?;
    let handled = window
        .update(cx, |_, window, cx| {
            strokes
                .into_iter()
                .map(|stroke| window.dispatch_keystroke(stroke, cx))
                .collect::<Vec<bool>>()
        })
        .map_err(RpcError::app)?;
    Ok(json!({ "handled": handled }))
}

/// Type text into whatever holds focus, one keystroke per character riding
/// the same simulated-IME path a test window uses: a binding may eat a
/// character, and the rest land through the input handler. Newlines go as
/// enter so multi-line fields and confirm-on-enter both behave.
fn type_text(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("type takes {\"text\": \"..\", \"window\"?}"))?;
    let window = target(params, cx)?;
    window
        .update(cx, |_, window, cx| {
            for ch in text.chars() {
                let stroke = match ch {
                    '\n' => Keystroke {
                        modifiers: Modifiers::default(),
                        key: "enter".into(),
                        key_char: None,
                    },
                    ch => Keystroke {
                        modifiers: Modifiers::default(),
                        key: ch.to_string(),
                        key_char: Some(ch.to_string()),
                    },
                };
                window.dispatch_keystroke(stroke, cx);
            }
        })
        .map_err(RpcError::app)?;
    Ok(json!({ "typed": text.chars().count() }))
}

/// Click at window-local logical coordinates: a move to get hover state
/// right, then down and up per click. `count` above one climbs the
/// click_count the double- and triple-click handlers key on.
fn click(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let position = position(params)?;
    let modifiers = modifiers(params);
    let button = match params
        .get("button")
        .and_then(Value::as_str)
        .unwrap_or("left")
    {
        "left" => MouseButton::Left,
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        other => {
            return Err(RpcError::invalid_params(format!(
                "unknown button {other:?}: left, right, or middle"
            )))
        }
    };
    let count = params
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 3) as usize;
    let window = target(params, cx)?;
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers,
                }),
                cx,
            );
            for click_count in 1..=count {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button,
                        position,
                        modifiers,
                        click_count,
                    }),
                    cx,
                );
            }
        })
        .map_err(RpcError::app)?;
    Ok(Value::Null)
}

/// Move the mouse to a point without pressing anything, for hover styles,
/// tooltips, and menus that open on entry.
fn hover(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let position = position(params)?;
    let modifiers = modifiers(params);
    let window = target(params, cx)?;
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers,
                }),
                cx,
            );
        })
        .map_err(RpcError::app)?;
    Ok(Value::Null)
}

/// Scroll at a point, `dx`/`dy` in lines the way a wheel notch counts:
/// positive y scrolls content up the way a wheel-up does.
fn scroll(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let position = position(params)?;
    let modifiers = modifiers(params);
    let dx = params.get("dx").and_then(Value::as_f64).unwrap_or(0.0);
    let dy = params.get("dy").and_then(Value::as_f64).unwrap_or(0.0);
    if dx == 0.0 && dy == 0.0 {
        return Err(RpcError::invalid_params(
            "scroll takes {\"x\", \"y\", \"dx\"?, \"dy\"?} with a nonzero delta",
        ));
    }
    let window = target(params, cx)?;
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Lines(point(dx as f32, dy as f32)),
                    modifiers,
                    touch_phase: TouchPhase::Moved,
                }),
                cx,
            );
        })
        .map_err(RpcError::app)?;
    Ok(Value::Null)
}

/// The `x`/`y` a mouse method lands on, in the window-local logical pixels
/// `debug.windows` reports sizes in.
fn position(params: &Value) -> Result<Point<Pixels>, RpcError> {
    let x = params.get("x").and_then(Value::as_f64);
    let y = params.get("y").and_then(Value::as_f64);
    match (x, y) {
        (Some(x), Some(y)) => Ok(point(px(x as f32), px(y as f32))),
        _ => Err(RpcError::invalid_params(
            "takes {\"x\": px, \"y\": px} in window-local logical pixels",
        )),
    }
}

/// Held modifiers for a mouse method, from an optional `modifiers` object:
/// `{"ctrl": true}` for a ctrl-click multi-select, and so on. `cmd` means
/// the platform key the keymap calls cmd.
fn modifiers(params: &Value) -> Modifiers {
    let flags = &params["modifiers"];
    let on = |name: &str| flags.get(name).and_then(Value::as_bool).unwrap_or(false);
    Modifiers {
        control: on("ctrl"),
        alt: on("alt"),
        shift: on("shift"),
        platform: on("cmd"),
        function: on("fn"),
    }
}
