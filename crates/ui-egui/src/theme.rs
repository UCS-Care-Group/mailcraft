// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Design tokens (light and dark), UI fonts and the egui style.

use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Stroke, Visuals};

/// MailCraft's own colour, the teal of its icon (and its darker ink for text on light
/// backgrounds). The interface itself is neutral greys with a blue accent: see [`Tokens`].
pub const APP_COLOR: Color32 = Color32::from_rgb(0x0B, 0x7F, 0xAB);
pub const APP_INK: Color32 = Color32::from_rgb(0x08, 0x62, 0x88);

/// The interface theme setting (File › Options › General): `System`, which follows the OS (or
/// browser) light/dark appearance live and is what a new installation starts with, or a fixed
/// light or dark palette.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Appearance {
    Light,
    Dark,
    #[default]
    System,
}

impl Appearance {
    /// In menu order.
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    /// The value used in `ui.json` and by the `ui.theme` command.
    pub fn code(self) -> &'static str {
        match self {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
            Appearance::System => "system",
        }
    }

    pub fn from_code(code: &str) -> Option<Appearance> {
        Appearance::ALL.into_iter().find(|a| a.code().eq_ignore_ascii_case(code.trim()))
    }

    /// The English label (translated with `tl!` where shown).
    pub fn label(self) -> &'static str {
        match self {
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
            Appearance::System => "Use system setting",
        }
    }

    /// The egui theme preference, with "Use system setting" settled to the theme the system
    /// reports for the main window (`system`; light when it reports none). egui is never left on
    /// `System` itself: it would settle that for each window from the window's own input, and a
    /// window of its own (a reminder, an opened item) has none of the desktop's preference on
    /// Linux, so it would be light beside a dark main window; two windows on different themes
    /// rebuild the font atlas they share every frame, which garbles the text of both.
    pub fn preference(self, system: Option<egui::Theme>) -> egui::ThemePreference {
        match self {
            Appearance::Light => egui::ThemePreference::Light,
            Appearance::Dark => egui::ThemePreference::Dark,
            Appearance::System => match system {
                Some(egui::Theme::Dark) => egui::ThemePreference::Dark,
                Some(egui::Theme::Light) | None => egui::ThemePreference::Light,
            },
        }
    }
}

/// An unknown saved value reads as the default rather than discarding the whole `ui.json`.
impl From<String> for Appearance {
    fn from(s: String) -> Self {
        Appearance::from_code(&s).unwrap_or_default()
    }
}

impl From<Appearance> for String {
    fn from(a: Appearance) -> Self {
        a.code().to_string()
    }
}

/// Interface size (File › Options › General, `ui.interfaceSize`), in percent: scales the ribbon,
/// menus, panes and dialogs on top of the display scale the system reports, through egui's zoom
/// factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(from = "serde_json::Value", into = "u16")]
pub struct InterfaceSize(u16);

impl InterfaceSize {
    pub const MIN: u16 = 80;
    pub const MAX: u16 = 200;
    pub const DEFAULT: InterfaceSize = InterfaceSize(100);
    /// The choices in File › Options, in percent.
    pub const PRESETS: [u16; 8] = [80, 90, 100, 110, 125, 150, 175, 200];

    /// A size from any number: rounded and clamped to 80–200 %; `None` for NaN or infinity.
    pub fn from_percent(percent: f64) -> Option<InterfaceSize> {
        percent.is_finite().then(|| InterfaceSize(percent.round().clamp(f64::from(Self::MIN), f64::from(Self::MAX)) as u16))
    }

    pub fn percent(self) -> u16 {
        self.0
    }

    /// The egui zoom factor (1.0 at 100 %).
    pub fn zoom_factor(self) -> f32 {
        f32::from(self.0) / 100.0
    }
}

impl Default for InterfaceSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A saved value that isn't a number reads as 100 %, and an out-of-range one is clamped, rather than
/// discarding the whole `ui.json`.
impl From<serde_json::Value> for InterfaceSize {
    fn from(v: serde_json::Value) -> Self {
        v.as_f64().and_then(InterfaceSize::from_percent).unwrap_or_default()
    }
}

impl From<InterfaceSize> for u16 {
    fn from(s: InterfaceSize) -> Self {
        s.0
    }
}

/// Every colour the UI uses; widgets never hard-code colours.
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub dark: bool,
    pub title_bar: Color32,
    pub ribbon: Color32,
    pub tab_strip: Color32,
    /// The module bar (Mail, Calendar, People, Tasks) down the left edge.
    pub nav: Color32,
    /// Behind the panes.
    pub canvas: Color32,
    /// The folder pane and other side panes.
    pub panel: Color32,
    /// The message list, the reading pane, calendar cells: where content sits.
    pub surface: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_disabled: Color32,
    pub icon: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub checked: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    /// Text/icon colour drawn on top of `accent`.
    pub on_accent: Color32,
    pub input: Color32,
    pub input_border: Color32,
    pub group_label: Color32,
    pub status_bar: Color32,
    /// A selected row or item.
    pub selection: Color32,
    /// Search hits.
    pub find_highlight: Color32,
    /// A flagged message's row in the message list: a faint tint, unlike a selected row or one
    /// under the pointer.
    pub flagged_row: Color32,
    /// The text of a flagged message whose follow-up is past its due date.
    pub overdue: Color32,
    /// Calendar: hours outside the working day, and days outside the month in view.
    pub off_hours: Color32,
    /// Calendar: today's column or cell.
    pub today: Color32,
    pub menu: Color32,
    pub blue: Color32,
    pub red: Color32,
    pub green: Color32,
    pub orange: Color32,
    /// The few colours an icon has besides its ink and the accent (a flag, a warning, the
    /// categories, a tick): each one at least 3:1 on every background an icon sits on.
    pub icon_red: Color32,
    pub icon_yellow: Color32,
    pub icon_green: Color32,
    pub icon_purple: Color32,
}

impl Tokens {
    /// The light palette: white and very light neutral greys, with a blue accent.
    ///
    /// The window's own background (title bar, tab strip, module bar, status bar, and what shows
    /// between the panes) is one grey; the ribbon and the panes sit on it as lighter sheets.
    pub fn light() -> Self {
        let window = Color32::from_rgb(0xF3, 0xF3, 0xF3);
        Tokens {
            dark: false,
            title_bar: window,
            ribbon: Color32::WHITE,
            tab_strip: window,
            nav: window,
            canvas: window,
            panel: Color32::from_rgb(0xFA, 0xFA, 0xFA),
            surface: Color32::WHITE,
            border: Color32::from_rgb(0xE0, 0xE0, 0xE0),
            border_strong: Color32::from_rgb(0xC4, 0xC4, 0xC4),
            text: Color32::from_rgb(0x24, 0x24, 0x24),
            text_dim: Color32::from_rgb(0x61, 0x61, 0x61),
            text_disabled: Color32::from_rgb(0xA6, 0xA6, 0xA6),
            icon: Color32::from_rgb(0x42, 0x42, 0x42),
            hover: Color32::from_rgb(0xE9, 0xE9, 0xE9),
            pressed: Color32::from_rgb(0xD6, 0xD6, 0xD6),
            checked: Color32::from_rgb(0xD6, 0xE8, 0xF8),
            accent: Color32::from_rgb(0x0F, 0x6C, 0xBD),
            accent_text: Color32::from_rgb(0x11, 0x5E, 0xA3),
            on_accent: Color32::WHITE,
            input: Color32::WHITE,
            input_border: Color32::from_rgb(0xC4, 0xC4, 0xC4),
            group_label: Color32::from_rgb(0x70, 0x70, 0x70),
            status_bar: window,
            selection: Color32::from_rgb(0xDE, 0xDE, 0xDE),
            find_highlight: Color32::from_rgba_unmultiplied(0xFF, 0xE0, 0x3D, 0x90),
            flagged_row: Color32::from_rgb(0xFC, 0xF0, 0xF1),
            overdue: Color32::from_rgb(0xB4, 0x23, 0x18),
            off_hours: Color32::from_rgb(0xF5, 0xF5, 0xF5),
            today: Color32::from_rgb(0xEB, 0xF3, 0xFC),
            menu: Color32::WHITE,
            blue: Color32::from_rgb(0x2F, 0x6F, 0xD0),
            red: Color32::from_rgb(0xD1, 0x3B, 0x3B),
            green: Color32::from_rgb(0x2E, 0x8B, 0x57),
            orange: Color32::from_rgb(0xE0, 0x7B, 0x1F),
            icon_red: Color32::from_rgb(0xC4, 0x31, 0x4B),
            icon_yellow: Color32::from_rgb(0x9C, 0x67, 0x00),
            icon_green: Color32::from_rgb(0x2B, 0x7A, 0x4B),
            icon_purple: Color32::from_rgb(0x7B, 0x4F, 0xB8),
        }
    }
    /// The dark palette: near-black neutral greys, with a light blue accent.
    ///
    /// The window's own background is the darkest, and one colour everywhere it shows; the ribbon
    /// is a lighter band on it, the folder pane nearly as dark as the window, the lists a step up.
    pub fn dark() -> Self {
        let window = Color32::from_rgb(0x0F, 0x16, 0x1F);
        Tokens {
            dark: true,
            title_bar: window,
            ribbon: Color32::from_rgb(0x29, 0x29, 0x29),
            tab_strip: window,
            nav: window,
            canvas: window,
            panel: Color32::from_rgb(0x14, 0x14, 0x14),
            surface: Color32::from_rgb(0x26, 0x26, 0x26),
            border: Color32::from_rgb(0x44, 0x44, 0x44),
            border_strong: Color32::from_rgb(0x66, 0x66, 0x66),
            text: Color32::WHITE,
            text_dim: Color32::from_rgb(0xC8, 0xC8, 0xC8),
            text_disabled: Color32::from_rgb(0x70, 0x70, 0x70),
            // A shade off white: line drawings in pure white glare on the dark panes.
            icon: Color32::from_rgb(0xE0, 0xE0, 0xE0),
            hover: Color32::from_rgb(0x38, 0x38, 0x38),
            pressed: Color32::from_rgb(0x4A, 0x4A, 0x4A),
            checked: Color32::from_rgb(0x1F, 0x3B, 0x57),
            accent: Color32::from_rgb(0x58, 0xB8, 0xFE),
            accent_text: Color32::from_rgb(0x82, 0xC7, 0xFF),
            on_accent: Color32::from_rgb(0x0A, 0x18, 0x26),
            input: Color32::from_rgb(0x1F, 0x1F, 0x1F),
            input_border: Color32::from_rgb(0x66, 0x66, 0x66),
            group_label: Color32::from_rgb(0xAD, 0xAD, 0xAD),
            status_bar: window,
            selection: Color32::from_rgb(0x44, 0x44, 0x44),
            find_highlight: Color32::from_rgba_unmultiplied(0xC8, 0xA8, 0x00, 0x70),
            flagged_row: Color32::from_rgb(0x3A, 0x2B, 0x2E),
            overdue: Color32::from_rgb(0xFF, 0x99, 0x99),
            off_hours: Color32::from_rgb(0x1F, 0x1F, 0x1F),
            today: Color32::from_rgb(0x1F, 0x32, 0x46),
            menu: Color32::from_rgb(0x29, 0x29, 0x29),
            blue: Color32::from_rgb(0x6C, 0x9B, 0xEA),
            red: Color32::from_rgb(0xEE, 0x6A, 0x6A),
            green: Color32::from_rgb(0x5B, 0xC0, 0x8A),
            orange: Color32::from_rgb(0xF0, 0xA0, 0x50),
            icon_red: Color32::from_rgb(0xF0, 0x7A, 0x7A),
            icon_yellow: Color32::from_rgb(0xE8, 0xB7, 0x4A),
            icon_green: Color32::from_rgb(0x5B, 0xC0, 0x8A),
            icon_purple: Color32::from_rgb(0xB6, 0x9B, 0xEE),
        }
    }
    pub fn get(ctx: &egui::Context) -> Tokens {
        if ctx.global_style().visuals.dark_mode { Tokens::dark() } else { Tokens::light() }
    }
}

/// The most of a system font file that is read: the faces wanted are about a megabyte each.
#[cfg(not(target_arch = "wasm32"))]
const SYSTEM_FONT_CAP: u64 = 8 * 1024 * 1024;

/// How far the system face's glyphs are moved up, as a fraction of the font size. The interface
/// centres a line of text by its line box, and was drawn with Inter, whose capitals sit in the
/// middle of that box; Segoe UI's sit lower, by this much.
#[cfg(not(target_arch = "wasm32"))]
const SYSTEM_FONT_RAISE: f32 = 0.064;

/// The system's own interface font, read from the system's fonts folder when the program starts:
/// on Windows, Segoe UI (regular, semibold and bold). It is never bundled, copied or downloaded.
/// A face that is missing, unreadable, too big or not a font is left out, and without the regular
/// face none is used: the bundled Inter then draws everything, as it does on other systems and in
/// the browser.
#[derive(Clone, Default)]
pub struct SystemFont {
    regular: Option<Arc<FontData>>,
    semibold: Option<Arc<FontData>>,
    bold: Option<Arc<FontData>>,
}

impl SystemFont {
    /// No system font: the bundled fonts alone.
    pub fn none() -> Self {
        SystemFont::default()
    }

    /// What this system has, read once and kept for the life of the program.
    pub fn load() -> Self {
        static LOADED: std::sync::OnceLock<SystemFont> = std::sync::OnceLock::new();
        LOADED.get_or_init(Self::read).clone()
    }

    #[cfg(windows)]
    fn read() -> Self {
        // `WINDIR` is how the system names its own folder; `SystemRoot` is the older name for it.
        ["WINDIR", "SystemRoot"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|root| std::path::PathBuf::from(root).join("Fonts"))
            .find(|dir| dir.is_dir())
            .map(|dir| Self::from_dir(&dir))
            .unwrap_or_default()
    }

    #[cfg(not(windows))]
    fn read() -> Self {
        SystemFont::none()
    }

    /// Segoe UI's faces as a Windows fonts folder names them, from `dir`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_dir(dir: &std::path::Path) -> Self {
        let face = |file: &str| {
            let tweak = egui::FontTweak { y_offset_factor: -SYSTEM_FONT_RAISE, ..Default::default() };
            read_font(&dir.join(file), SYSTEM_FONT_CAP).map(|bytes| Arc::new(FontData::from_owned(bytes).tweak(tweak)))
        };
        match face("segoeui.ttf") {
            Some(regular) => SystemFont { regular: Some(regular), semibold: face("seguisb.ttf"), bold: face("segoeuib.ttf") },
            None => SystemFont::none(),
        }
    }

    /// Whether the interface is drawn in the system's font rather than the bundled one.
    pub fn is_present(&self) -> bool {
        self.regular.is_some()
    }
}

/// A font file's bytes, when it is there, no bigger than `cap` bytes and laid out as a font.
#[cfg(not(target_arch = "wasm32"))]
fn read_font(path: &std::path::Path, cap: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len == 0 || len > cap {
        return None;
    }
    let mut bytes = Vec::with_capacity(usize::try_from(len).ok()?);
    // The cap again: the file may have grown since it was measured.
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= cap && is_font(&bytes)).then_some(bytes)
}

/// Whether `bytes` are laid out as one TrueType or OpenType face with the tables text needs, all of
/// them inside the file. egui panics on font data it cannot open, so nothing read from a disk goes
/// to it unchecked.
#[cfg(not(target_arch = "wasm32"))]
fn is_font(bytes: &[u8]) -> bool {
    let u16_at = |at: usize| bytes.get(at..at.checked_add(2)?).and_then(|b| <[u8; 2]>::try_from(b).ok()).map(u16::from_be_bytes);
    let u32_at = |at: usize| bytes.get(at..at.checked_add(4)?).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes);
    // TrueType outlines, `OTTO` (CFF outlines) or Apple's `true`: what the font reader takes.
    if !matches!(u32_at(0), Some(0x0001_0000 | 0x4F54_544F | 0x7472_7565)) {
        return false;
    }
    let Some(tables) = u16_at(4) else { return false };
    const NEEDED: [&[u8; 4]; 5] = [b"cmap", b"head", b"hhea", b"hmtx", b"maxp"];
    let mut found = [false; NEEDED.len()];
    for i in 0..usize::from(tables) {
        // The directory: twelve bytes, then sixteen for each table (tag, checksum, offset, length).
        let record = 12 + i * 16;
        let (Some(tag), Some(offset), Some(length)) = (bytes.get(record..record + 4), u32_at(record + 8), u32_at(record + 12)) else {
            return false;
        };
        if u64::from(offset) + u64::from(length) > bytes.len() as u64 {
            return false;
        }
        for (seen, needed) in found.iter_mut().zip(NEEDED) {
            *seen |= tag == needed.as_slice();
        }
    }
    found.iter().all(|seen| *seen)
}

/// Install the interface fonts: the system's own where there is one ([`SystemFont`]), then the
/// bundled Inter for what it lacks and for everywhere else, Source Sans 3 and JetBrains Mono.
pub fn install_fonts(ctx: &egui::Context) {
    install_fonts_with(ctx, &SystemFont::load());
}

/// [`install_fonts`] with a system font of the caller's choosing ([`SystemFont::none`] draws the
/// interface in MailCraft's own fonts).
pub fn install_fonts_with(ctx: &egui::Context, system: &SystemFont) {
    ctx.set_fonts(font_definitions_with(system));
}

/// The interface fonts as [`install_fonts`] installs them on this system.
pub fn font_definitions() -> FontDefinitions {
    font_definitions_with(&SystemFont::load())
}

/// The interface fonts with `system` in front of the bundled ones.
pub fn font_definitions_with(system: &SystemFont) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "Inter", include_bytes!("../../../assets/fonts/Inter-Regular.ttf"));
    add(&mut fonts, "InterMedium", include_bytes!("../../../assets/fonts/Inter-Medium.ttf"));
    add(&mut fonts, "InterSemiBold", include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"));
    add(&mut fonts, "SourceSans", include_bytes!("../../../assets/fonts/SourceSans3-Regular.ttf"));
    add(&mut fonts, "Mono", include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"));
    // The system's faces, each under its own name, when there is a regular one to go with them.
    let mut system_face = |name: &str, face: &Option<Arc<FontData>>| -> Option<String> {
        let face = face.as_ref().filter(|_| system.is_present())?;
        fonts.font_data.insert(name.into(), face.clone());
        Some(name.to_string())
    };
    let system_regular = system_face("SystemRegular", &system.regular);
    let system_semibold = system_face("SystemSemibold", &system.semibold);
    let system_bold = system_face("SystemBold", &system.bold);
    // Symbols and emoji fall back to egui's defaults (kept after our faces).
    let mut fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fallback.push("SourceSans".into());
    // Last, JetBrains Mono, for what only it has: U+FFFD among them, the mark of a character that
    // couldn't be read, which would otherwise be an empty box (#180).
    fallback.push("Mono".into());
    // A weight is the system's face of that weight, then the bundled face nearest to it, then
    // the regular faces for whatever glyph those two lack.
    let chain = |faces: &[Option<&str>]| -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for name in faces.iter().flatten().map(|n| n.to_string()).chain(fallback.iter().cloned()) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    };
    let (regular, semibold, bold) = (system_regular.as_deref(), system_semibold.as_deref(), system_bold.as_deref());
    fonts.families.insert(FontFamily::Proportional, chain(&[regular, Some("Inter")]));
    // Segoe UI has no medium: its semibold is the lighter of the two heavier weights it has.
    fonts.families.insert(FontFamily::Name("medium".into()), chain(&[semibold, Some("InterMedium"), regular, Some("Inter")]));
    fonts.families.insert(FontFamily::Name("semibold".into()), chain(&[semibold, Some("InterSemiBold"), regular, Some("Inter")]));
    // Inter comes in no bold here: its semibold stands in.
    fonts.families.insert(FontFamily::Name("bold".into()), chain(&[bold, semibold, Some("InterSemiBold"), regular, Some("Inter")]));
    if let Some(m) = fonts.families.get_mut(&FontFamily::Monospace) {
        m.insert(0, "Mono".into());
    }
    fonts
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}
/// The heaviest weight: a selected folder's name, unread counts.
pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}
pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// A UI type rung: size, weight pairing, and size-specific letter-spacing (large text is
/// tightened, small text opened). Use these instead of raw `regular(size)`/`semibold(size)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TypeRung {
    /// Tab strip, title bar, centred window title.
    Chrome,
    /// Ribbon buttons, menus, dialogs.
    Control,
    /// Group labels, status bar, tooltips.
    Caption,
}

impl TypeRung {
    /// Font size in points.
    pub fn size(self) -> f32 {
        match self {
            TypeRung::Chrome => 12.5,
            TypeRung::Control => 11.5,
            TypeRung::Caption => 11.0,
        }
    }
    /// Extra letter-spacing in points (em-relative, so it scales with the size).
    pub fn tracking(self) -> f32 {
        match self {
            TypeRung::Chrome => -0.012 * self.size(),
            TypeRung::Control => 0.0,
            TypeRung::Caption => 0.018 * self.size(),
        }
    }
    /// Line height in points; tighter leading as the text grows.
    pub fn line_height(self) -> f32 {
        match self {
            TypeRung::Chrome => 1.25 * self.size(),
            TypeRung::Control => 1.42 * self.size(),
            TypeRung::Caption => 1.5 * self.size(),
        }
    }

    pub fn regular(self) -> FontId {
        regular(self.size())
    }
    pub fn medium(self) -> FontId {
        medium(self.size())
    }
    pub fn semibold(self) -> FontId {
        semibold(self.size())
    }
}

/// Width in points of one line of `tracking`-spaced text. `layout_no_wrap` ignores
/// letter-spacing, so non-zero tracking goes through an explicit `LayoutJob`.
pub fn text_width(painter: &egui::Painter, text: &str, font: &FontId, tracking: f32, color: Color32) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    if tracking.abs() <= 1e-6 {
        return painter.layout_no_wrap(text.to_string(), font.clone(), color).size().x;
    }
    painter.layout_job(tracked_job(text, font, tracking, color)).size().x
}

/// Paint one line of tracked text with the given anchor and return its rect.
pub fn paint_text(painter: &egui::Painter, pos: egui::Pos2, anchor: Align2, text: &str, font: &FontId, tracking: f32, color: Color32) -> Rect {
    if tracking.abs() <= 1e-6 {
        return painter.text(pos, anchor, text.to_string(), font.clone(), color);
    }
    let galley = painter.layout_job(tracked_job(text, font, tracking, color));
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

/// A single-section, no-wrap `LayoutJob` carrying `tracking`.
fn tracked_job(text: &str, font: &FontId, tracking: f32, color: Color32) -> egui::text::LayoutJob {
    let format = egui::text::TextFormat { font_id: font.clone(), extra_letter_spacing: tracking, line_height: None, color, ..Default::default() };
    egui::text::LayoutJob::single_section(text.to_string(), format)
}

/// One line of text cut to `width` with an ellipsis, painted with its left edge at `pos`
/// (vertically centred on it). Returns the width used.
pub fn paint_clipped(painter: &egui::Painter, pos: egui::Pos2, width: f32, text: &str, font: FontId, color: Color32) -> f32 {
    if width <= 4.0 || text.is_empty() {
        return 0.0;
    }
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::text::TextFormat { font_id: font, color, ..Default::default() });
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = painter.layout_job(job);
    let size = galley.size();
    painter.galley(egui::pos2(pos.x, pos.y - size.y / 2.0), galley, color);
    size.x
}

/// Apply the interface theme to egui. Both palettes go in, our light one as egui's light style
/// and our dark one as its dark style, and `preference` ([`Appearance::preference`]) pins egui
/// (and the window decorations) to one of them.
pub fn apply(ctx: &egui::Context, preference: egui::ThemePreference) {
    ctx.set_visuals_of(egui::Theme::Light, visuals(&Tokens::light()));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(&Tokens::dark()));
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(8.0, 3.0);
        s.spacing.interact_size = egui::vec2(24.0, 22.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.text_styles.insert(egui::TextStyle::Body, regular(12.5));
        s.text_styles.insert(egui::TextStyle::Button, regular(12.5));
        s.text_styles.insert(egui::TextStyle::Small, regular(10.5));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(17.0));
        s.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
        s.interaction.tooltip_delay = 0.45;
    });
    ctx.options_mut(|o| {
        o.fallback_theme = egui::Theme::Light;
        o.theme_preference = preference;
    });
}

/// egui's visuals for one of our palettes.
fn visuals(t: &Tokens) -> Visuals {
    let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.ribbon;
    v.window_fill = t.menu;
    v.extreme_bg_color = t.input;
    v.faint_bg_color = t.panel;
    v.window_stroke = Stroke::new(1.0, t.border_strong);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = t.accent.linear_multiply(if t.dark { 0.45 } else { 0.30 });
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.bg_fill = t.input;
    v.widgets.inactive.weak_bg_fill = t.input;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.input_border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.corner_radius = CornerRadius::same(4);
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, t.border_strong);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.corner_radius = CornerRadius::same(4);
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.active.corner_radius = CornerRadius::same(4);
    v.widgets.open.bg_fill = t.pressed;
    v.widgets.open.weak_bg_fill = t.pressed;
    v.popup_shadow = egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark { 120 } else { 40 }) };
    v.window_shadow = egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if t.dark { 140 } else { 50 }) };
    v
}

pub fn c32(c: mailcraft_model::Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// A stable colour for a person's avatar, from their address.
pub fn avatar_color(key: &str, t: &Tokens) -> Color32 {
    // Our own palette; mid-tones that carry white initials in both themes.
    const PALETTE: [(u8, u8, u8); 8] = [
        (0x0B, 0x7F, 0xAB),
        (0x2E, 0x8B, 0x57),
        (0xB3, 0x5C, 0x1E),
        (0x7E, 0x57, 0xC2),
        (0xB0, 0x3A, 0x5B),
        (0x3F, 0x6F, 0xB5),
        (0x6B, 0x7A, 0x1F),
        (0x8A, 0x5A, 0x44),
    ];
    let hash = key.to_lowercase().bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193));
    let (r, g, b) = PALETTE.get(hash as usize % PALETTE.len()).copied().unwrap_or((0x0B, 0x7F, 0xAB));
    let c = Color32::from_rgb(r, g, b);
    if t.dark { c.linear_multiply(0.9) } else { c }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{SystemFont, Tokens};

    /// A folder to stand in for a system's fonts folder, emptied and removed when dropped.
    struct FontsDir(std::path::PathBuf);

    impl FontsDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mailcraft-fonts-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            FontsDir(dir)
        }
        fn put(&self, file: &str, bytes: &[u8]) {
            std::fs::write(self.0.join(file), bytes).unwrap();
        }
    }

    impl Drop for FontsDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A font that is certainly one: the bundled Inter, standing in for a system's face.
    const A_FONT: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");

    /// Lay out a line in every weight with `fonts`: egui panics on font data it cannot open.
    fn draws(fonts: egui::FontDefinitions) -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for font in [super::regular(12.0), super::medium(12.0), super::semibold(12.0), super::bold(12.0), super::mono(12.0)] {
                let width = ui.painter().layout_no_wrap("Inbox [12] ›".to_string(), font, egui::Color32::WHITE).size().x;
                assert!(width > 20.0);
            }
        })
        .drop_without_applying_deltas();
        ctx
    }

    /// #180: U+FFFD, the mark of a character that couldn't be read, has a glyph in every weight
    /// of the interface's text, from JetBrains Mono at the end of each family.
    #[test]
    fn the_replacement_character_is_drawn() {
        let fonts = super::font_definitions_with(&SystemFont::none());
        for (name, faces) in &fonts.families {
            if *name != egui::FontFamily::Monospace {
                assert_eq!(faces.last().map(String::as_str), Some("Mono"), "{name:?}");
            }
        }
        let ctx = draws(fonts);
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for font in [super::regular(12.0), super::medium(12.0), super::semibold(12.0), super::bold(12.0)] {
                assert!(ui.fonts_mut(|f| f.has_glyph(&font, '\u{fffd}')), "{font:?}");
            }
        })
        .drop_without_applying_deltas();
    }

    /// Without the system's font (another system, the browser, a folder that isn't there) the
    /// bundled Inter draws everything, in every weight.
    #[test]
    fn a_missing_system_font_falls_back_to_the_bundled_one() {
        let missing = SystemFont::from_dir(std::path::Path::new("no-such-folder-anywhere").join("Fonts").as_path());
        assert!(!missing.is_present());
        assert!(!SystemFont::none().is_present());
        let fonts = super::font_definitions_with(&missing);
        assert!(!fonts.font_data.keys().any(|name| name.starts_with("System")));
        let first = |family: egui::FontFamily| fonts.families.get(&family).and_then(|f| f.first()).cloned().unwrap();
        assert_eq!(first(egui::FontFamily::Proportional), "Inter");
        assert_eq!(first(egui::FontFamily::Name("medium".into())), "InterMedium");
        assert_eq!(first(egui::FontFamily::Name("semibold".into())), "InterSemiBold");
        assert_eq!(first(egui::FontFamily::Name("bold".into())), "InterSemiBold");
        draws(fonts);

        // The semibold and bold faces alone are not a font to draw the interface in.
        let dir = FontsDir::new("no-regular");
        dir.put("seguisb.ttf", A_FONT);
        dir.put("segoeuib.ttf", A_FONT);
        assert!(!SystemFont::from_dir(&dir.0).is_present());
    }

    /// The system's faces go in front of the bundled ones, which stay behind them for the glyphs
    /// they lack; a weight the system doesn't have is the bundled face of that weight.
    #[test]
    fn the_system_font_goes_in_front_of_the_bundled_one() {
        let dir = FontsDir::new("regular-only");
        dir.put("segoeui.ttf", A_FONT);
        let system = SystemFont::from_dir(&dir.0);
        assert!(system.is_present());
        let fonts = super::font_definitions_with(&system);
        let family = |family: egui::FontFamily| fonts.families.get(&family).cloned().unwrap();
        let proportional = family(egui::FontFamily::Proportional);
        assert_eq!(proportional.iter().take(2).collect::<Vec<_>>(), ["SystemRegular", "Inter"]);
        assert!(proportional.iter().any(|name| name == "SourceSans"), "the fallbacks stay: {proportional:?}");
        assert_eq!(family(egui::FontFamily::Name("semibold".into())).iter().take(3).collect::<Vec<_>>(), ["InterSemiBold", "SystemRegular", "Inter"]);
        // Every name a family lists has its data: egui panics on one that has none.
        for (name, faces) in &fonts.families {
            for face in faces {
                assert!(fonts.font_data.contains_key(face), "{name:?} lists {face}, which has no data");
            }
        }
        draws(fonts);

        let dir = FontsDir::new("all-weights");
        for file in ["segoeui.ttf", "seguisb.ttf", "segoeuib.ttf"] {
            dir.put(file, A_FONT);
        }
        let fonts = super::font_definitions_with(&SystemFont::from_dir(&dir.0));
        let first = |family: &str| fonts.families.get(&egui::FontFamily::Name(family.into())).and_then(|f| f.first()).cloned().unwrap();
        assert_eq!((first("medium"), first("semibold"), first("bold")), ("SystemSemibold".into(), "SystemSemibold".into(), "SystemBold".into()));
        draws(fonts);
    }

    /// A file in the fonts folder that isn't a font (or is far too big to be the one wanted) is
    /// left out rather than handed to egui, which panics on what it cannot open.
    #[test]
    fn a_file_that_is_not_a_font_is_left_out() {
        assert!(super::is_font(A_FONT));
        let cases: [(&str, Vec<u8>); 7] = [
            ("empty", Vec::new()),
            ("text", b"This is not a font, whatever it is called.".to_vec()),
            ("short", vec![0, 1, 0, 0]),
            // The right first bytes and a table count, and nothing after them.
            ("header only", vec![0, 1, 0, 0, 0xFF, 0xFF, 0, 0, 0, 0, 0, 0]),
            // A font cut off inside its tables: the directory names what isn't there.
            ("cut off", A_FONT.get(..A_FONT.len() / 2).unwrap().to_vec()),
            ("cut off in the directory", A_FONT.get(..40).unwrap().to_vec()),
            // A collection of faces is not what is asked for by these names.
            ("collection", [b"ttcf".as_slice(), A_FONT.get(4..).unwrap()].concat()),
        ];
        for (what, bytes) in &cases {
            assert!(!super::is_font(bytes), "{what}");
            let dir = FontsDir::new("not-a-font");
            dir.put("segoeui.ttf", bytes);
            let system = SystemFont::from_dir(&dir.0);
            assert!(!system.is_present(), "{what}");
            draws(super::font_definitions_with(&system));
        }
        // A font with no tables for text in it: every table it names is there, the ones needed are not.
        let mut renamed = A_FONT.to_vec();
        let tables = usize::from(u16::from_be_bytes([renamed[4], renamed[5]]));
        for i in 0..tables {
            let tag = 12 + i * 16;
            if &renamed[tag..tag + 4] == b"cmap" {
                renamed[tag..tag + 4].copy_from_slice(b"zzzz");
            }
        }
        assert!(!super::is_font(&renamed));
        // The cap on how much is read.
        let dir = FontsDir::new("too-big");
        dir.put("segoeui.ttf", A_FONT);
        assert!(super::read_font(&dir.0.join("segoeui.ttf"), A_FONT.len() as u64).is_some());
        assert!(super::read_font(&dir.0.join("segoeui.ttf"), A_FONT.len() as u64 - 1).is_none());
        assert!(super::read_font(&dir.0.join("segoeui.ttf"), 0).is_none());
        assert!(super::read_font(&dir.0.join("no-such-file.ttf"), u64::MAX).is_none());
    }

    /// What this system has, whatever that is, draws; and where it has Segoe UI, that is in front.
    #[test]
    fn the_fonts_of_this_system_draw() {
        let system = SystemFont::load();
        let fonts = super::font_definitions();
        let first = fonts.families.get(&egui::FontFamily::Proportional).and_then(|f| f.first()).cloned().unwrap();
        assert_eq!(first, if system.is_present() { "SystemRegular" } else { "Inter" });
        draws(fonts);
    }

    /// Relative luminance of a colour (WCAG 2).
    fn luminance(c: egui::Color32) -> f32 {
        let channel = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Text can be read on every background it is drawn on, in both palettes; and the window's
    /// own colour is one colour wherever it shows.
    #[test]
    fn text_stays_readable_on_every_background() {
        for t in [Tokens::light(), Tokens::dark()] {
            let theme = if t.dark { "dark" } else { "light" };
            let backgrounds = [
                ("title_bar", t.title_bar),
                ("ribbon", t.ribbon),
                ("tab_strip", t.tab_strip),
                ("nav", t.nav),
                ("canvas", t.canvas),
                ("panel", t.panel),
                ("surface", t.surface),
                ("status_bar", t.status_bar),
                ("menu", t.menu),
                ("input", t.input),
                ("hover", t.hover),
                ("selection", t.selection),
                ("checked", t.checked),
                ("off_hours", t.off_hours),
                ("today", t.today),
                ("flagged_row", t.flagged_row),
            ];
            for (name, bg) in backgrounds {
                for (ink, color) in [("text", t.text), ("text_dim", t.text_dim), ("accent_text", t.accent_text), ("icon", t.icon)] {
                    assert!(contrast(color, bg) >= 4.5, "{theme}: {ink} on {name} is {:.2}", contrast(color, bg));
                }
                // Marks, unread counts and what is disabled need less than running text does.
                assert!(contrast(t.accent, bg) >= 3.0, "{theme}: accent on {name} is {:.2}", contrast(t.accent, bg));
                assert!(contrast(t.text, bg) > contrast(t.text_disabled, bg), "{theme}: disabled text on {name}");
                // An icon's colours are marks, and need what the accent needs.
                for (hue, color) in
                    [("icon_red", t.icon_red), ("icon_yellow", t.icon_yellow), ("icon_green", t.icon_green), ("icon_purple", t.icon_purple)]
                {
                    assert!(contrast(color, bg) >= 3.0, "{theme}: {hue} on {name} is {:.2}", contrast(color, bg));
                }
            }
            assert!(contrast(t.on_accent, t.accent) >= 4.5, "{theme}: on_accent");
            assert!(contrast(t.on_accent, t.accent_text) >= 4.5, "{theme}: on_accent on a hovered button");
            assert!(contrast(t.text, t.pressed) >= 4.5, "{theme}: text on pressed");
            assert!(contrast(t.group_label, t.ribbon) >= 4.5, "{theme}: group labels");
            assert!(contrast(t.red, t.surface) >= 4.5, "{theme}: red");
            for bg in [t.flagged_row, t.surface, t.hover, t.selection] {
                assert!(contrast(t.overdue, bg) >= 4.5, "{theme}: overdue on {bg:?} is {:.2}", contrast(t.overdue, bg));
            }
            // A flagged row is told apart from the list, a selected row and one under the pointer.
            for other in [t.surface, t.selection, t.hover, t.checked, t.today] {
                assert_ne!(t.flagged_row, other, "{theme}: flagged row");
            }
            // A selected row and one under the pointer are told apart from each other and from the pane.
            for (a, b) in [(t.selection, t.panel), (t.selection, t.surface), (t.selection, t.hover), (t.hover, t.panel), (t.hover, t.ribbon)] {
                assert_ne!(a, b, "{theme}");
            }
            assert!([t.tab_strip, t.nav, t.canvas, t.status_bar].iter().all(|c| *c == t.title_bar), "{theme}: the window's own colour");
            assert_ne!(t.ribbon, t.title_bar, "{theme}: the ribbon is a band on the window");
        }
        // No cast in the greys: the panes are neutral in both palettes.
        for t in [Tokens::light(), Tokens::dark()] {
            for c in [t.ribbon, t.panel, t.surface, t.border, t.border_strong, t.hover, t.pressed, t.selection, t.menu, t.input] {
                assert!(c.r() == c.g() && c.g() == c.b(), "{c:?}");
            }
        }
    }

    /// Symbols written as text in the interface code must exist in the interface fonts (the
    /// system's font where there is one, Inter and egui's emoji fallbacks behind it, and those
    /// alone everywhere else); a missing glyph draws as an empty box.
    #[test]
    fn interface_symbols_have_glyphs() {
        for fonts in [super::font_definitions(), super::font_definitions_with(&SystemFont::none())] {
            symbols_have_glyphs(fonts);
        }
    }

    /// Every character of every shipped catalogue (accents, ß, « » „ “ ¿ ¡, the narrow no-break
    /// space French puts before a colon) has a glyph in every weight of the bundled fonts.
    #[test]
    fn the_catalogues_are_drawn_with_the_bundled_fonts() {
        let catalogues = [
            ("fr", include_str!("../../i18n/po/fr.po")),
            ("de", include_str!("../../i18n/po/de.po")),
            ("es", include_str!("../../i18n/po/es.po")),
            ("it", include_str!("../../i18n/po/it.po")),
        ];
        let mut chars = BTreeSet::new();
        for (code, text) in catalogues {
            chars.extend(text.chars().filter(|c| !c.is_ascii() && !c.is_control()).map(|c| (c, code)));
        }
        assert!(chars.iter().any(|(c, _)| *c == '\u{202f}') && chars.iter().any(|(c, _)| *c == 'ß'));
        let ctx = draws(super::font_definitions_with(&SystemFont::none()));
        ctx.run_ui(egui::RawInput::default(), |ui| {
            for font in [super::regular(12.0), super::medium(12.0), super::semibold(12.0), super::bold(12.0)] {
                let missing: Vec<String> = ui.fonts_mut(|f| {
                    chars.iter().filter(|(c, _)| !f.has_glyph(&font, *c)).map(|(c, code)| format!("{c} U+{:04X} in {code}.po", *c as u32)).collect()
                });
                assert!(missing.is_empty(), "{font:?}: {missing:#?}");
            }
        })
        .drop_without_applying_deltas();
    }

    fn symbols_have_glyphs(fonts: egui::FontDefinitions) {
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut symbols = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let code = std::fs::read_to_string(&path).unwrap();
            // Characters inside string literals, line by line (good enough for this code base).
            for line in code.lines().filter(|l| !l.trim_start().starts_with("//")) {
                for (i, part) in line.split('"').enumerate() {
                    if i % 2 == 1 {
                        symbols.extend(
                            part.chars()
                                .filter(|c| !c.is_ascii() && !c.is_alphabetic() && !c.is_whitespace())
                                .map(|c| (c, path.file_name().unwrap().to_string_lossy().to_string())),
                        );
                    }
                }
            }
        }
        let font = super::regular(12.5);
        let missing: Vec<String> = ctx.fonts_mut(|f| {
            symbols.iter().filter(|(c, _)| !f.has_glyph(&font, *c)).map(|(c, file)| format!("{c} U+{:04X} in {file}", *c as u32)).collect()
        });
        assert!(missing.is_empty(), "symbols without a glyph: {missing:#?}");
    }

    #[test]
    fn saved_settings_read_leniently() {
        use super::{Appearance, InterfaceSize};
        assert_eq!(serde_json::from_str::<Appearance>("\"DARK\"").unwrap(), Appearance::Dark);
        // An unknown value falls back to following the system, like a new installation.
        assert_eq!(serde_json::from_str::<Appearance>("\"sepia\"").unwrap(), Appearance::System);
        assert_eq!(serde_json::from_str::<InterfaceSize>("125").unwrap().percent(), 125);
        assert_eq!(serde_json::from_str::<InterfaceSize>("9000").unwrap().percent(), 200);
        assert_eq!(serde_json::from_str::<InterfaceSize>("\"big\"").unwrap().percent(), 100);
        assert_eq!(InterfaceSize::from_percent(f64::NAN), None);
    }

    /// "Use system setting" is settled from the main window's input: a window of its own (a
    /// reminder) whose input says nothing of the desktop's preference, as on Linux, has the same
    /// theme, so the two don't rebuild the font atlas they share every frame, garbling the text.
    #[test]
    fn a_window_of_its_own_has_the_main_windows_theme() {
        let ctx = egui::Context::default();
        super::apply(&ctx, super::Appearance::System.preference(Some(egui::Theme::Dark)));
        ctx.run_ui(egui::RawInput { system_theme: None, ..Default::default() }, |_| {}).textures_delta.clear();
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert_eq!(super::Appearance::System.preference(None), egui::ThemePreference::Light);
    }
}
