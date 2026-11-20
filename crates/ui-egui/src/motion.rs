// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Animations (#1183): File › Options › General › Animations plays them, stops them, or follows
//! the system's setting (Windows' Animation effects, read by the desktop app through
//! [`crate::Services::animations_enabled`]; where the system says nothing, they play). Off, egui's
//! own animations (menus, collapsing sections, hover fades) take no time, scrolling to a place
//! jumps there, and the search bar over the list appears at once. A spinner that says something is
//! still going on keeps turning: it is news, not decoration.

/// How often the system's setting is asked again while the window runs, in seconds.
const RECHECK: f64 = 5.0;

/// The owner's choice, kept in `ui.json` by its code; anything else read there is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Motion {
    /// As the system says (the default).
    #[default]
    System,
    On,
    Off,
}

impl Motion {
    pub const ALL: [Motion; 3] = [Motion::System, Motion::On, Motion::Off];

    /// The value `ui.animations` takes and gives.
    pub fn code(self) -> &'static str {
        match self {
            Motion::System => "system",
            Motion::On => "on",
            Motion::Off => "off",
        }
    }

    pub fn from_code(code: &str) -> Option<Motion> {
        Motion::ALL.into_iter().find(|m| m.code() == code.trim())
    }

    /// Its name in Options (English; the caller translates it).
    pub fn label(self) -> &'static str {
        match self {
            Motion::System => "Use system setting",
            Motion::On => "Always play",
            Motion::Off => "Never play",
        }
    }

    /// Whether animations play, with `system` the system's answer (`None`: it has none).
    pub fn plays(self, system: Option<bool>) -> bool {
        match self {
            Motion::System => system.unwrap_or(true),
            Motion::On => true,
            Motion::Off => false,
        }
    }
}

impl<'de> serde::Deserialize<'de> for Motion {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = <serde_json::Value as serde::Deserialize>::deserialize(d)?;
        Ok(v.as_str().and_then(Motion::from_code).unwrap_or_default())
    }
}

impl serde::Serialize for Motion {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

/// The system's answer as last asked, and what egui was last told.
#[derive(Default)]
pub struct Tracker {
    /// When the system was last asked (egui's clock), and what it said.
    asked: Option<(f64, Option<bool>)>,
    /// Whether animations play, as last installed in egui's styles.
    pub(crate) applied: Option<bool>,
    /// What the styles had before animations were stopped, given back when they play again.
    kept: Option<(f32, egui::style::ScrollAnimation)>,
}

impl Tracker {
    /// Whether animations play now, with `choice` the owner's: the system is asked (every few
    /// seconds at most) only while the choice is to follow it.
    pub fn plays(&mut self, choice: Motion, now: f64, ask: Option<&dyn Fn() -> bool>) -> bool {
        let system = if choice == Motion::System { self.system(now, ask) } else { None };
        choice.plays(system)
    }

    /// Tells egui's styles whether animations play, when that changed. While they play, the styles
    /// are left as they are: stopping them keeps what they had, and playing again gives it back.
    pub fn apply(&mut self, ctx: &egui::Context, plays: bool) {
        if self.applied == Some(plays) {
            return;
        }
        self.applied = Some(plays);
        let set = |time: f32, scroll: egui::style::ScrollAnimation| {
            ctx.all_styles_mut(|s| {
                s.animation_time = time;
                s.scroll_animation = scroll;
            });
        };
        if plays {
            if let Some((time, scroll)) = self.kept.take() {
                set(time, scroll);
            }
        } else {
            if self.kept.is_none() {
                let style = ctx.global_style();
                self.kept = Some((style.animation_time, style.scroll_animation));
            }
            set(0.0, egui::style::ScrollAnimation::none());
        }
    }

    /// The system's answer, asked again every few seconds at most.
    pub fn system(&mut self, now: f64, ask: Option<&dyn Fn() -> bool>) -> Option<bool> {
        let ask = ask?;
        match self.asked {
            Some((at, answer)) if now >= at && now - at < RECHECK => answer,
            _ => {
                let answer = Some(ask());
                self.asked = Some((now, answer));
                answer
            }
        }
    }
}

/// How long a fade of `seconds` takes in `ui` now: none when animations are off.
pub fn fade(ui: &egui::Ui, seconds: f32) -> f32 {
    if ui.style().animation_time > 0.0 { seconds } else { 0.0 }
}
