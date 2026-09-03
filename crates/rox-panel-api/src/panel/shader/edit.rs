//! What the in-app shader editor works on: one surface's source, where an
//! applied buffer goes back to, and how one editor window tells surfaces
//! apart so a second Edit focuses the window already open on it.
//!
//! The editor lives in the binary and opens through the openers table, so
//! this is the shape that crosses that boundary. A surface builds a target
//! from its own config, hands it up, and the window never learns whether
//! it's over a panel, the screen, or a pool entry beyond what the key says.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, EntityId, SharedString};

use super::{approve, ProgramCtx};

/// How an applied source reaches its surface. Takes the text and the app
/// handle a window action has.
pub type Write = Arc<dyn Fn(String, &mut App)>;

/// Which surface a window is over. The registry key: one window per key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditKey {
    /// A workspace shader, by name. Several surfaces can wear one entry,
    /// and an apply lands on all of them, so they share a window too.
    Pool(String),
    /// A panel's own inline source, keyed by the panel.
    Panel(EntityId),
    /// The screen shader's inline source.
    Screen,
    /// The backdrop shader's inline source.
    Backdrop,
}

/// One surface's source as the editor takes it, and the way back.
pub struct ShaderEditTarget {
    pub key: EditKey,
    /// What the window's header calls the surface: the panel's name, the
    /// pool entry's, or the screen.
    pub title: SharedString,
    /// The text as it runs now, which is the buffer's starting point and
    /// the first revert point.
    pub source: String,
    /// Where the program's images resolve from, so a check compiles what
    /// the surface itself would.
    pub ctx: ProgramCtx,
    /// The file the source is bookmarked to, if any. An apply writes it
    /// too, so the working copy and what runs don't drift apart and the
    /// next external edit still starts from the applied text.
    pub path: Option<PathBuf>,
    /// Put an applied source into the surface. Runs after the approval
    /// and the file write, with the app handle a window action has.
    pub write: Write,
}

impl ShaderEditTarget {
    /// A target over a pool entry, or None when the pool doesn't hold the
    /// name. Applies write the entry, so every surface on the name
    /// follows, and the entry's bookmark takes the file write.
    pub fn pool(name: &str) -> Option<ShaderEditTarget> {
        let entry = rox_core::settings::shader_pool_get(name)?;
        let name = name.to_string();
        let ctx = ProgramCtx::named(&name);
        Some(ShaderEditTarget {
            key: EditKey::Pool(name.clone()),
            title: name.clone().into(),
            source: entry.source,
            ctx,
            path: entry.path,
            write: Arc::new(move |source, _| {
                let mut pool = rox_core::settings::shader_pool();
                if let Some(entry) = pool.iter_mut().find(|entry| entry.name == name) {
                    entry.source = source;
                }
                rox_core::settings::set_shader_pool(pool);
            }),
        })
    }

    /// Put an edited buffer where it goes. The text approves first, since
    /// applying it is the user vouching for it the way picking a file is;
    /// then the bookmarked file takes it, then the surface. A file that
    /// won't take the write doesn't hold the apply up, because the source
    /// is what runs and the file is only its working copy: the surface
    /// still gets the text, and the failure comes back as a line for the
    /// window's readout.
    pub fn apply(&self, source: String, cx: &mut App) -> Option<String> {
        approve(&source);
        let warning = self.path.as_ref().and_then(|path| {
            std::fs::write(path, &source)
                .err()
                .map(|error| format!("writing {}: {error}", path.display()))
        });
        (self.write)(source, cx);
        warning
    }
}
