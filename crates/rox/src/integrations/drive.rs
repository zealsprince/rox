//! The debug scope's drive half (ADR 22): synthetic input and action
//! dispatch over the control socket. Everything goes through gpui's own event
//! pipeline, so it works on any platform and compositor.
//!
//! Coordinates are window-local logical pixels, from `debug.windows`.

use gpui::{
    App, Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent, TouchPhase, point,
    px,
};
use serde_json::{Value, json};

use rox_ipc::RpcError;

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

/// The window named by `window`, else the active one, else the first.
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

/// [`target`] for input a person only gives in the window they're in. A
/// compositor with no keyboard on its seat never activates a window, and the
/// frontmost-workspace lookups behind the app-wide actions would then stay on
/// whichever opened last, so this notes the window as in front the way
/// activation does.
fn target_in_front(params: &Value, cx: &mut App) -> Result<gpui::AnyWindowHandle, RpcError> {
    let window = target(params, cx)?;
    rox_panel_api::windows::note_activated(window, cx);

    Ok(window)
}

/// Registration only says the name builds, not that anything is bound.
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

fn action(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
        RpcError::invalid_params("action takes {\"name\", \"data\"?, \"window\"?}")
    })?;
    let data = params.get("data").filter(|data| !data.is_null()).cloned();
    let action = cx
        .build_action(name, data)
        .map_err(|err| RpcError::app(format!("can't build {name}: {err}")))?;
    let window = target_in_front(params, cx)?;
    window
        .update(cx, |_, window, cx| window.dispatch_action(action, cx))
        .map_err(RpcError::app)?;
    Ok(Value::Null)
}

/// Keystrokes in gpui keymap syntax, space separated. Returns per stroke
/// whether anything handled it.
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
    let window = target_in_front(params, cx)?;
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

/// One keystroke per character over the simulated-IME path; newlines go as
/// enter.
fn type_text(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::invalid_params("type takes {\"text\": \"..\", \"window\"?}"))?;
    let window = target_in_front(params, cx)?;
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

/// A move first so hover state is right; `count` climbs `click_count`.
/// `phase` "down" or "up" sends only that half, so a drag is a down, moves
/// with `hover`'s `button`, and an up.
fn click(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let position = position(params)?;
    let modifiers = modifiers(params);
    let button = button(params)?.unwrap_or(MouseButton::Left);
    let phase = params.get("phase").and_then(Value::as_str);
    let pressed_button = (phase == Some("up")).then_some(button);
    let count = params
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 3) as usize;
    let window = target_in_front(params, cx)?;
    window
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button,
                    modifiers,
                }),
                cx,
            );
            for click_count in 1..=count {
                if phase != Some("up") {
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
                }
                if phase != Some("down") {
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
            }
        })
        .map_err(RpcError::app)?;
    Ok(Value::Null)
}

/// `button` moves with that button held, the middle of a drag.
/// With `leave`, the pointer leaves the window from there instead, as a
/// compositor reports it: an exit and no move.
fn hover(params: &Value, cx: &mut App) -> Result<Value, RpcError> {
    let position = position(params)?;
    let modifiers = modifiers(params);
    let pressed_button = button(params)?;
    let leave = params.get("leave").and_then(Value::as_bool) == Some(true);
    let window = target(params, cx)?;
    window
        .update(cx, |_, window, cx| {
            let event = if leave {
                PlatformInput::MouseExited(MouseExitEvent {
                    position,
                    pressed_button,
                    modifiers,
                })
            } else {
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button,
                    modifiers,
                })
            };
            window.dispatch_event(event, cx);
        })
        .map_err(RpcError::app)?;
    Ok(Value::Null)
}

/// `dx`/`dy` in wheel lines; positive y scrolls content up.
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

fn button(params: &Value) -> Result<Option<MouseButton>, RpcError> {
    match params.get("button").and_then(Value::as_str) {
        None => Ok(None),
        Some("left") => Ok(Some(MouseButton::Left)),
        Some("right") => Ok(Some(MouseButton::Right)),
        Some("middle") => Ok(Some(MouseButton::Middle)),
        Some(other) => Err(RpcError::invalid_params(format!(
            "unknown button {other:?}: left, right, or middle"
        ))),
    }
}

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

/// `cmd` is the platform key.
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
