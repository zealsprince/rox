//! rox's own documents arriving from outside: workspaces, plugins, objects
//! and `rox://` links, from a launch, a second launch, a macOS open event or
//! a drop on a window. A document from outside is untrusted, so opening one
//! lists it and asks: nothing runs, applies or switches on by itself.
//!
//! A workspace goes through the same import and apply confirm the menu
//! shows, a plugin through the same install the Plugins page runs.
//! Objects and links go to handlers the canvas and the catalog set.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use gpui::{AnyWindowHandle, App, Context, Window};
use rox_library::open_files::Document;
use rox_panel_api::panel;
use rox_panel_api::toast::Toast;
use rox_plugins::package;
use rox_services::plugins as host;

use crate::workspace::Workspace;

/// What arrived before any workspace window was up to take it.
static PENDING: Mutex<Vec<Document>> = Mutex::new(Vec::new());

static OBJECT_HANDLER: OnceLock<fn(PathBuf, &mut App)> = OnceLock::new();
static LINK_HANDLER: OnceLock<fn(&str, &mut App)> = OnceLock::new();

/// A workspace is imported and offered through the apply confirm. A plugin
/// is installed and the Plugins page opens on it. An object goes to the
/// object handler, a link to the link handler. Queues until a workspace
/// window exists.
pub(crate) fn open(documents: Vec<Document>, cx: &mut App) {
    if documents.is_empty() {
        return;
    }

    if rox_panel_api::windows::front_workspace(cx).is_none() {
        PENDING
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend(documents);
        return;
    }

    crate::workspace::with_front_workspace(cx, move |ws, window, cx| {
        open_in(ws, documents, window, cx);
    });
}

/// Open what queued while no workspace window was up. Called as one opens.
pub(crate) fn drain(cx: &mut App) {
    let pending = std::mem::take(&mut *PENDING.lock().unwrap_or_else(PoisonError::into_inner));
    open(pending, cx);
}

/// [`open`] against one workspace, for a drop on its window.
pub(crate) fn open_in(
    ws: &mut Workspace,
    documents: Vec<Document>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    for document in documents {
        match document {
            Document::Workspace(path) => ws.import_workspace_file(path, window, cx),

            Document::Plugin(path) => install_plugin(path, window.window_handle(), cx),

            // Deferred: a handler reaches into the canvas, which may sit in
            // this workspace, and this runs inside its update.
            Document::Object(path) => match OBJECT_HANDLER.get().copied() {
                Some(handler) => cx.defer(move |cx| handler(path, cx)),
                None => log::info!("documents: nothing opens objects yet: {}", path.display()),
            },

            Document::Link(link) => match LINK_HANDLER.get().copied() {
                Some(handler) => cx.defer(move |cx| handler(&link, cx)),
                None => log::info!("documents: nothing opens links yet: {link}"),
            },
        }
    }
}

/// Set once objects install. Until then an object is logged and dropped.
pub(crate) fn set_object_handler(handler: fn(PathBuf, &mut App)) {
    let _ = OBJECT_HANDLER.set(handler);
}

/// Set once the catalog reads links. Until then a link is logged and
/// dropped.
pub(crate) fn set_link_handler(handler: fn(&str, &mut App)) {
    let _ = LINK_HANDLER.set(handler);
}

/// The object handler: installed, then placed in the canvas that last had
/// focus in the front workspace, through the place confirm when its card has
/// something to review. With no canvas in the layout it stays installed, and
/// a toast says it needs one.
pub(crate) fn open_object(path: PathBuf, cx: &mut App) {
    crate::workspace::with_front_workspace(cx, move |ws, window, cx| {
        let name = match crate::objects::install(&path) {
            Ok(name) => name,
            Err(e) => {
                log::warn!("documents: installing {}: {e}", path.display());
                Toast::new(panel::Tone::Bad, e)
                    .title(rox_i18n::t!(
                        "canvas-object-add-failed",
                        file = file_name(&path)
                    ))
                    .show(window, cx);
                return;
            }
        };

        match crate::objects::canvas_for(ws, cx) {
            // Outside this workspace update: a place with nothing to review
            // edits the canvas at once, and that reaches back into the
            // workspace, which gpui refuses while it's mid-update.
            Some(canvas) => window.defer(cx, move |window, cx| {
                canvas.update(cx, |canvas, cx| {
                    crate::objects::offer(canvas, &name, window, cx)
                })
            }),
            None => Toast::new(
                panel::Tone::Info,
                rox_i18n::t!("canvas-object-needs-canvas", name = name.as_str()),
            )
            .show(window, cx),
        }
    });
}

/// Install off the UI thread, the way the Plugins page does: the hash and
/// the copy both scale with the archive. The page opens on success so the
/// new row is in view. Switching it on stays the user's call.
fn install_plugin(path: PathBuf, origin: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        let archive = path.clone();
        let installed = cx
            .background_executor()
            .spawn(async move {
                let peek = package::peek(&archive)?;
                host::install_file(&archive)?;

                Ok::<_, String>(peek.manifest.name)
            })
            .await;

        cx.update(|cx| {
            let toast = match installed {
                Ok(name) => {
                    host::rescan(cx);
                    origin
                        .update(cx, |_, window, cx| {
                            rox_panel_api::panel_settings::open_app_page(
                                "settings-page-plugins",
                                window,
                                cx,
                            );
                        })
                        .ok();

                    Toast::new(
                        panel::Tone::Good,
                        rox_i18n::t!("settings-plugins-installed", name = name),
                    )
                }
                Err(e) => {
                    log::warn!("documents: installing {}: {e}", path.display());
                    Toast::new(panel::Tone::Bad, e).title(file_name(&path))
                }
            };
            toast.post(origin, cx);
        })
        .ok();
    })
    .detach();
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
