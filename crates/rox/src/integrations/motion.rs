//! The desktop's reduced-motion preference, which Reduce Motion's System
//! choice follows ([`settings::reduce_motion`]).
//!
//! Linux asks the desktop portal's Settings interface, on the session bus rox
//! already talks to, with `ReadOne`, a plain read that never raises a dialog:
//! `org.freedesktop.appearance` `reduced-motion` where the portal's backend
//! offers it, else GNOME's `enable-animations`. Windows reads the Show
//! Animations switch. macOS isn't read, so there the setting is On or Off.
//!
//! A missing bus, a missing portal or a key no backend offers all read as no
//! preference, quietly. The read runs at startup and again whenever a
//! workspace window comes to the front, so a change made in the desktop's
//! settings lands when the user comes back.

use gpui::App;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use rox_core::settings;

/// Read the preference again and hand it to the settings static.
pub(crate) fn reread(cx: &mut App) {
    platform::read(cx);
}

#[cfg(target_os = "linux")]
mod platform {
    use std::sync::Mutex;

    use gpui::App;
    use zbus::zvariant::{OwnedValue, Value};

    use super::settings;

    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const SETTINGS: &str = "org.freedesktop.portal.Settings";

    /// One connection for every read, opened on the first.
    static BUS: Mutex<Option<zbus::Connection>> = Mutex::new(None);

    pub(super) fn read(cx: &mut App) {
        let asked = cx.background_executor().spawn(ask());
        cx.spawn(async move |cx| {
            let reduces = asked.await.unwrap_or(false);
            cx.update(|cx| settings::note_system_reduces_motion(reduces, cx))
                .ok();
        })
        .detach();
    }

    async fn ask() -> Option<bool> {
        let conn = connection().await?;

        if let Some(value) = read_one(&conn, "org.freedesktop.appearance", "reduced-motion").await {
            return reduced_motion(&value);
        }

        let value = read_one(&conn, "org.gnome.desktop.interface", "enable-animations").await?;
        animations_enabled(&value).map(|on| !on)
    }

    async fn connection() -> Option<zbus::Connection> {
        let held = BUS.lock().unwrap().clone();
        if held.is_some() {
            return held;
        }

        match zbus::Connection::session().await {
            Ok(conn) => {
                *BUS.lock().unwrap() = Some(conn.clone());
                Some(conn)
            }
            Err(e) => {
                log::debug!("reduce motion: no session bus, no system preference: {e}");
                None
            }
        }
    }

    async fn read_one(conn: &zbus::Connection, namespace: &str, key: &str) -> Option<OwnedValue> {
        let reply = conn
            .call_method(
                Some(PORTAL),
                PATH,
                Some(SETTINGS),
                "ReadOne",
                &(namespace, key),
            )
            .await
            .inspect_err(|e| log::debug!("reduce motion: no {namespace} {key} here: {e}"))
            .ok()?;

        reply.body().deserialize::<OwnedValue>().ok()
    }

    /// A value can arrive wrapped in a second variant, as the portal's older
    /// `Read` answers.
    fn inner<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
        match value {
            Value::Value(boxed) => inner(boxed),
            other => other,
        }
    }

    /// `1` asks for reduced motion. `0` and anything unknown is no preference.
    pub(super) fn reduced_motion(value: &Value<'_>) -> Option<bool> {
        match inner(value) {
            Value::U32(n) => Some(*n == 1),
            _ => None,
        }
    }

    pub(super) fn animations_enabled(value: &Value<'_>) -> Option<bool> {
        match inner(value) {
            Value::Bool(on) => Some(*on),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_portal_key_reads_one_as_reduce_and_anything_else_as_none() {
            assert_eq!(reduced_motion(&Value::U32(1)), Some(true));
            assert_eq!(reduced_motion(&Value::U32(0)), Some(false));
            assert_eq!(reduced_motion(&Value::U32(7)), Some(false));
            assert_eq!(
                reduced_motion(&Value::Value(Box::new(Value::U32(1)))),
                Some(true)
            );
            assert_eq!(reduced_motion(&Value::Bool(true)), None);
        }

        #[test]
        fn gnome_reduces_motion_when_animations_are_off() {
            assert_eq!(animations_enabled(&Value::Bool(false)), Some(false));
            assert_eq!(animations_enabled(&Value::Bool(true)), Some(true));
            assert_eq!(animations_enabled(&Value::U32(0)), None);
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use gpui::App;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    use windows_sys::core::BOOL;

    use super::settings;

    pub(super) fn read(cx: &mut App) {
        let mut on: BOOL = 1;
        // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL through pvParam,
        // which points at `on` for the length of the call.
        let read = unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut on as *mut BOOL).cast(),
                0,
            )
        };
        settings::note_system_reduces_motion(read != 0 && on == 0, cx);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use gpui::App;

    pub(super) fn read(_: &mut App) {}
}
