//! What the in-app shader editor works on: one surface's source, where an
//! applied buffer goes back to, and the key that gives each surface one
//! window. The editor lives in the binary and opens through the openers
//! table; this is the shape that crosses that boundary.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, EntityId, SharedString};

use super::{ProgramCtx, approve};

pub type Write = Arc<dyn Fn(String, &mut App)>;

/// The registry key: one editor window per key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditKey {
    /// A workspace shader by name. Every surface wearing it shares a window.
    Pool(String),
    Panel(EntityId),
    Screen,
    Backdrop,
}

pub struct ShaderEditTarget {
    pub key: EditKey,
    pub title: SharedString,
    /// The text as it runs now: the buffer's start and first revert point.
    pub source: String,
    /// Where the program's images resolve from, so a check compiles what
    /// the surface itself would.
    pub ctx: ProgramCtx,
    /// The bookmarked file. An apply writes it too, so the next external
    /// edit starts from the applied text.
    pub path: Option<PathBuf>,
    /// Runs after the approval and the file write.
    pub write: Write,
}

impl ShaderEditTarget {
    /// A target over a pool entry. Applies write the entry, so every
    /// surface on the name follows.
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

    /// Approve, write the bookmarked file, then the surface. Applying is
    /// the user vouching for the text, like picking a file. A failed file
    /// write doesn't block the apply; it comes back as a readout line.
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
