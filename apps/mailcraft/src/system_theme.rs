//! The system's light or dark appearance, for File › Options › Interface theme "Use system
//! setting", where the windowing library doesn't say it.
//!
//! On Windows and macOS winit reports the system's theme, and changes to it, and egui follows.
//! On Linux and the BSDs winit 0.30 reports nothing (`ActiveEventLoop::system_theme` and
//! `Window::theme` are `None`, and no `ThemeChanged` ever comes), so egui's `System` preference
//! fell back to light whatever the desktop said. There the desktop says it through the
//! freedesktop.org settings portal (`org.freedesktop.appearance` / `color-scheme`: 0 no
//! preference, read as light, 1 dark, 2 light), which GNOME, KDE and the others implement: it is read once on a
//! background thread, and then followed through the portal's `SettingChanged` signal, each
//! change waking the window. Over D-Bus with `zbus` (already in the tree); nothing leaves the
//! computer. Where there is no portal the theme stays unknown and egui's fallback (light) holds.
//!
//! The browser build doesn't need this: eframe reads `prefers-color-scheme` and follows it.

use std::sync::atomic::{AtomicU8, Ordering};

/// What the portal said last: [`UNKNOWN`], [`DARK`] or [`LIGHT`].
static SCHEME: AtomicU8 = AtomicU8::new(UNKNOWN);
/// The theme last handed to the window's own decorations under "Use system setting" (same codes;
/// [`UNKNOWN`] when none was, or a fixed theme was chosen since).
static DECORATIONS: AtomicU8 = AtomicU8::new(UNKNOWN);

const UNKNOWN: u8 = 0;
const DARK: u8 = 1;
const LIGHT: u8 = 2;

fn encode(theme: Option<egui::Theme>) -> u8 {
    match theme {
        None => UNKNOWN,
        Some(egui::Theme::Dark) => DARK,
        Some(egui::Theme::Light) => LIGHT,
    }
}

fn decode(code: u8) -> Option<egui::Theme> {
    match code {
        DARK => Some(egui::Theme::Dark),
        LIGHT => Some(egui::Theme::Light),
        _ => None,
    }
}

/// The portal's `color-scheme` value as a theme: 1 dark; 2 light, and 0 ("no preference", what
/// GNOME's Default style says) light too, the interface's own default; anything else unknown.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd", test)),
    allow(dead_code)
)]
fn scheme(value: u32) -> Option<egui::Theme> {
    match value {
        1 => Some(egui::Theme::Dark),
        0 | 2 => Some(egui::Theme::Light),
        _ => None,
    }
}

/// The system's theme as this frame should see it: what the windowing library reported when it
/// reported anything, else what the portal said.
fn resolve(reported: Option<egui::Theme>, portal: Option<egui::Theme>) -> Option<egui::Theme> {
    reported.or(portal)
}

/// Start following the desktop's appearance (Linux and the BSDs; elsewhere nothing to do).
/// `ctx` is woken whenever it changes.
pub fn start(ctx: &egui::Context) {
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
    portal::start(ctx.clone());
    #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
    let _ = ctx;
}

/// Fill in the system's theme in a frame's input when the windowing library left it out.
pub fn fill(raw: &mut egui::RawInput) {
    raw.system_theme = resolve(raw.system_theme, decode(SCHEME.load(Ordering::Relaxed)));
}

/// Under "Use system setting", have the window's own title bar (the decorations winit draws on
/// Wayland, the theme hint on X11) follow the portal's theme too: egui only tells the window
/// "the system's default", which the decorations read once.
pub fn decorations(ctx: &egui::Context) {
    let want = decoration_theme(ctx.options(|o| o.theme_preference), decode(SCHEME.load(Ordering::Relaxed)));
    let code = encode(want);
    if DECORATIONS.swap(code, Ordering::Relaxed) != code
        && let Some(theme) = want
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match theme {
            egui::Theme::Dark => egui::SystemTheme::Dark,
            egui::Theme::Light => egui::SystemTheme::Light,
        }));
    }
}

/// The theme to hand the decorations: the portal's, while the setting follows the system; none
/// (egui's own command stands) for a fixed theme or while the portal hasn't said.
fn decoration_theme(preference: egui::ThemePreference, portal: Option<egui::Theme>) -> Option<egui::Theme> {
    match preference {
        egui::ThemePreference::System => portal,
        egui::ThemePreference::Light | egui::ThemePreference::Dark => None,
    }
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
mod portal {
    use std::sync::atomic::Ordering;

    use zbus::zvariant::Value;

    const DESTINATION: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const INTERFACE: &str = "org.freedesktop.portal.Settings";
    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";

    pub fn start(ctx: egui::Context) {
        let spawned = std::thread::Builder::new().name("system-theme".into()).spawn(move || {
            if let Err(e) = follow(&ctx) {
                log::debug!("system theme: the settings portal can't be read: {e}");
            }
        });
        if let Err(e) = spawned {
            log::debug!("system theme: {e}");
        }
    }

    fn set(ctx: &egui::Context, value: &Value<'_>) {
        let code = super::encode(color_scheme(value).and_then(super::scheme));
        if super::SCHEME.swap(code, Ordering::Relaxed) != code {
            ctx.request_repaint();
        }
    }

    /// Read the setting, then follow its changes until the bus goes away.
    fn follow(ctx: &egui::Context) -> zbus::Result<()> {
        let connection = zbus::blocking::Connection::session()?;
        let proxy = zbus::blocking::Proxy::new(&connection, DESTINATION, PATH, INTERFACE)?;
        // Listening first, so that a change between the read and the listening isn't missed.
        let changes = proxy.receive_signal("SettingChanged")?;
        // `ReadOne` (portal version 2) answers the value; the older `Read` wraps it once more.
        let first: zbus::Result<zbus::zvariant::OwnedValue> =
            proxy.call("ReadOne", &(NAMESPACE, KEY)).or_else(|_| proxy.call("Read", &(NAMESPACE, KEY)));
        match first {
            Ok(value) => set(ctx, &value),
            Err(e) => log::debug!("system theme: no color-scheme setting: {e}"),
        }
        for message in changes {
            let Ok((namespace, key, value)) = message.body().deserialize::<(String, String, zbus::zvariant::OwnedValue)>() else {
                continue;
            };
            if namespace == NAMESPACE && key == KEY {
                set(ctx, &value);
            }
        }
        Ok(())
    }

    /// The number in a `color-scheme` answer, through the variants it may be wrapped in (a few at
    /// most: the bus is the session's, but its answers are still taken with care).
    pub(super) fn color_scheme(value: &Value<'_>) -> Option<u32> {
        let mut value = value;
        for _ in 0..4 {
            match value {
                Value::Value(inner) => value = inner,
                Value::U32(n) => return Some(*n),
                _ => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_portal_scheme_reads_as_a_theme() {
        assert_eq!(scheme(1), Some(egui::Theme::Dark));
        assert_eq!(scheme(2), Some(egui::Theme::Light));
        assert_eq!(scheme(0), Some(egui::Theme::Light), "no preference");
        assert_eq!(scheme(7), None);
        for theme in [None, Some(egui::Theme::Dark), Some(egui::Theme::Light)] {
            assert_eq!(decode(encode(theme)), theme);
        }
    }

    /// The bug: on Linux winit reports no theme, and "Use system setting" stayed light on a dark
    /// desktop. The portal's answer now fills in, and a theme the windowing library does report
    /// (Windows, macOS) still wins.
    #[test]
    fn the_portal_fills_in_what_the_window_does_not_report() {
        assert_eq!(resolve(None, Some(egui::Theme::Dark)), Some(egui::Theme::Dark));
        assert_eq!(resolve(Some(egui::Theme::Light), Some(egui::Theme::Dark)), Some(egui::Theme::Light));
        assert_eq!(resolve(None, None), None);
        // And egui then draws dark under "Use system setting".
        let ctx = egui::Context::default();
        ctx.options_mut(|o| o.theme_preference = egui::ThemePreference::System);
        let mut raw = egui::RawInput::default();
        raw.system_theme = resolve(raw.system_theme, Some(egui::Theme::Dark));
        let mut output = ctx.run_ui(raw, |_| {});
        output.textures_delta.clear();
        assert_eq!(ctx.theme(), egui::Theme::Dark);
    }

    #[test]
    fn the_decorations_follow_only_under_the_system_setting() {
        let dark = Some(egui::Theme::Dark);
        assert_eq!(decoration_theme(egui::ThemePreference::System, dark), dark);
        assert_eq!(decoration_theme(egui::ThemePreference::Light, dark), None);
        assert_eq!(decoration_theme(egui::ThemePreference::Dark, Some(egui::Theme::Light)), None);
        assert_eq!(decoration_theme(egui::ThemePreference::System, None), None);
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
    #[test]
    fn a_color_scheme_answer_is_unwrapped_with_care() {
        use zbus::zvariant::Value;
        assert_eq!(portal::color_scheme(&Value::U32(1)), Some(1));
        assert_eq!(portal::color_scheme(&Value::Value(Box::new(Value::U32(2)))), Some(2), "ReadOne");
        assert_eq!(portal::color_scheme(&Value::Value(Box::new(Value::Value(Box::new(Value::U32(1)))))), Some(1), "Read");
        assert_eq!(portal::color_scheme(&Value::Str("dark".into())), None);
        let mut deep = Value::U32(1);
        for _ in 0..10 {
            deep = Value::Value(Box::new(deep));
        }
        assert_eq!(portal::color_scheme(&deep), None);
    }
}
