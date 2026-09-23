//! The Development settings page: the experimental switch and what it turns
//! on.

use super::*;

impl SettingsWindow {
    fn set_experimental(&mut self, on: bool, cx: &mut Context<Self>) {
        self.experimental = on;
        Settings::update(move |s| s.experimental = on);
        settings::set_experimental(on, cx);
        // The in-window menus read the flag as they draw, so the refresh
        // above is enough for them; the macOS bar is built once and held by
        // the system, so it has to be rebuilt.
        crate::workspace::native_menu::rebuild(cx);
        cx.notify();
    }

    /// The Development page: the switches for work that isn't finished, and
    /// the controls for whatever they turn on.
    pub(super) fn development_page(&self, q: &Query, cx: &mut Context<Self>) -> PageBody {
        PageBody::new().section(Section::new(
            q,
            icons::FLASK,
            rox_i18n::t!("settings-development-section-features"),
            None,
            |rows| {
                rows.keyed(
                    "settings-development-experimental-panels",
                    &["debug", "beta", "unfinished"],
                    panel::toggle(self.experimental, Self::set_experimental, cx),
                )
            },
        ))
    }
}
