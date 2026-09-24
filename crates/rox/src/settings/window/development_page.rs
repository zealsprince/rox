//! The Development settings page: the experimental switch.

use super::*;

impl SettingsWindow {
    fn set_experimental(&mut self, on: bool, cx: &mut Context<Self>) {
        self.experimental = on;
        Settings::update(move |s| s.experimental = on);
        settings::set_experimental(on, cx);
        // The macOS bar is built once and held by the system, so it needs a
        // rebuild.
        crate::workspace::native_menu::rebuild(cx);
        cx.notify();
    }

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
