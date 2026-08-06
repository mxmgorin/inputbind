//! The SDL2 backend — the only module that names SDL. Everything above it
//! works in [`Pad`]s, [`Mods`] and normalized key names.

mod gamepad;
mod keymap;

pub use gamepad::Gamepad;
pub use keymap::Keymap;

use super::gesture::normalize;
use super::pad::Pad;
use super::Mods;
use sdl2::controller::{Axis, Button};
use sdl2::keyboard::{Keycode, Mod};

/// SDL's own key names, normalized, paired with their codes.
///
/// Built by asking SDL to name every keycode rather than by spelling names
/// here: a guessed spelling would either fail to resolve or, worse, resolve to
/// the wrong key. Built once at load — never on the input path.
pub struct KeyNames {
    /// Sorted by name for binary search.
    entries: Vec<(String, u32)>,
}

impl Default for KeyNames {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyNames {
    pub fn new() -> Self {
        /// SDL sets this bit on keycodes that stand for a scancode rather than
        /// a character (`SDLK_SCANCODE_MASK`).
        const SCANCODE_MASK: i32 = 1 << 30;
        /// `SDL_NUM_SCANCODES`; the tail is unassigned and simply goes unnamed.
        const SCANCODES: i32 = 512;
        /// Character keycodes are the characters themselves — including the
        /// control codes Return (13), Escape (27), Tab, Backspace and Delete,
        /// which is why this starts below the printable range.
        const ASCII: std::ops::RangeInclusive<i32> = 0..=127;

        let mut entries = Vec::new();
        let mut add = |code: i32| {
            let Some(keycode) = Keycode::from_i32(code) else {
                return;
            };
            let name = keycode.name();
            // Several codes can carry one name — SDL names 65 "A" though it
            // only ever delivers 97, and `Return` is both SDLK_RETURN and the
            // legacy SDLK_RETURN2. Keep the code SDL resolves the name back to
            // and the ambiguity is settled by SDL rather than by a guess.
            if name.is_empty() || Keycode::from_name(&name).map(i32::from) != Some(code) {
                return;
            }
            entries.push((normalize(&name), code as u32));
        };
        for code in ASCII {
            add(code);
        }
        for scancode in 1..SCANCODES {
            add(SCANCODE_MASK | scancode);
        }
        entries.sort_by(|(a, _), (b, _)| a.cmp(b));
        Self { entries }
    }

    /// The code for a normalized key name, or `None` if SDL has no such key.
    pub fn code(&self, name: &str) -> Option<u32> {
        let name = alias(name);
        self.entries
            .binary_search_by(|(known, _)| known.as_str().cmp(name))
            .ok()
            .map(|i| self.entries[i].1)
    }
}

/// Short spellings for keys whose SDL names are a mouthful. An alias resolves
/// through the derived table like any other name, so one that misses simply
/// fails to bind instead of binding the wrong key.
fn alias(name: &str) -> &str {
    match name {
        "esc" => "escape",
        "enter" => "return",
        "pgup" => "pageup",
        "pgdn" | "pgdown" => "pagedown",
        "del" => "delete",
        "ins" => "insert",
        "lctrl" => "leftctrl",
        "rctrl" => "rightctrl",
        "lshift" => "leftshift",
        "rshift" => "rightshift",
        "lalt" => "leftalt",
        "ralt" => "rightalt",
        other => other,
    }
}

/// The `i16` SDL reports for an axis, normalized to `-1.0..=1.0` (a trigger
/// only ever uses `0.0..=1.0`).
pub fn axis_value(value: i16) -> f32 {
    f32::from(value) / f32::from(i16::MAX)
}

/// The pad input a controller button stands for. The D-pad arrives as buttons
/// and maps to the four directions, and the stick clicks are L3/R3 — all bind
/// like any other input.
pub fn pad_of(button: Button) -> Option<Pad> {
    Some(match button {
        Button::A => Pad::A,
        Button::B => Pad::B,
        Button::X => Pad::X,
        Button::Y => Pad::Y,
        Button::LeftShoulder => Pad::L1,
        Button::RightShoulder => Pad::R1,
        Button::LeftStick => Pad::L3,
        Button::RightStick => Pad::R3,
        Button::Start => Pad::Start,
        Button::Back => Pad::Select,
        Button::DPadUp => Pad::Up,
        Button::DPadDown => Pad::Down,
        Button::DPadLeft => Pad::Left,
        Button::DPadRight => Pad::Right,
        _ => return None,
    })
}

/// The pad a trigger axis stands for. Triggers arrive as axes, so the caller
/// folds the normalized value through a [`Trigger`](super::Trigger) to get
/// presses; a stick's directional axes go through [`super::Stick`] instead.
pub fn trigger_of(axis: Axis) -> Option<Pad> {
    match axis {
        Axis::TriggerLeft => Some(Pad::L2),
        Axis::TriggerRight => Some(Pad::R2),
        _ => None,
    }
}

/// The modifier bit a key itself stands for, if it is one.
fn own_mod(keycode: Keycode) -> Option<Mods> {
    match keycode {
        Keycode::LCTRL | Keycode::RCTRL => Some(Mods::CTRL),
        Keycode::LALT | Keycode::RALT => Some(Mods::ALT),
        Keycode::LSHIFT | Keycode::RSHIFT => Some(Mods::SHIFT),
        _ => None,
    }
}

/// Whether this key is itself a modifier, which capture defers to its release:
/// alone it is the binding, with a second key it is that key's modifier.
pub fn is_modifier(keycode: Keycode) -> bool {
    own_mod(keycode).is_some()
}

/// The modifiers to look a key gesture up by. A modifier key reports itself as
/// held in its own press event, so its own bit is cleared — otherwise `leftctrl`
/// could never match a binding, only `ctrl+leftctrl`.
pub fn mods_for(keycode: Keycode, keymod: Mod) -> Mods {
    match own_mod(keycode) {
        Some(own) => mods_of(keymod).without(own),
        None => mods_of(keymod),
    }
}

/// Left and right modifier variants fold together.
pub fn mods_of(keymod: Mod) -> Mods {
    let mut mods = Mods::NONE;
    for (sdl, ours) in [
        (Mod::LCTRLMOD | Mod::RCTRLMOD, Mods::CTRL),
        (Mod::LALTMOD | Mod::RALTMOD, Mods::ALT),
        (Mod::LSHIFTMOD | Mod::RSHIFTMOD, Mods::SHIFT),
    ] {
        if keymod.intersects(sdl) {
            mods = mods.union(ours);
        }
    }
    mods
}

/// The code a key gesture is looked up by. Allocation-free, unlike
/// [`key_name`] — this is the one on the input path.
pub fn key_code(keycode: Keycode) -> u32 {
    i32::from(keycode) as u32
}

/// The normalized name capture writes into a gesture.
pub fn key_name(keycode: Keycode) -> String {
    normalize(&keycode.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdl_names_resolve_through_the_derived_table() {
        let names = KeyNames::new();
        // Letters, digits and function keys.
        for name in ["a", "z", "0", "f1", "f12"] {
            assert!(names.code(name).is_some(), "`{name}` should resolve");
        }
        // The multi-word names that a spelled-out table would have got wrong.
        for name in ["pagedown", "pageup", "leftctrl", "leftshift", "leftalt"] {
            assert!(names.code(name).is_some(), "`{name}` should resolve");
        }
        // Short aliases reach the same keys.
        assert_eq!(names.code("esc"), names.code("escape"));
        assert_eq!(names.code("enter"), names.code("return"));
        assert_eq!(names.code("lctrl"), names.code("leftctrl"));
        assert_eq!(names.code("pgdn"), names.code("pagedown"));
        // `-` survives normalization, so ctrl+- is bindable.
        assert!(names.code("-").is_some());
        assert_eq!(names.code("elbow"), None);
    }

    #[test]
    fn triggers_and_stick_clicks_reach_their_pads() {
        assert_eq!(trigger_of(Axis::TriggerLeft), Some(Pad::L2));
        assert_eq!(trigger_of(Axis::TriggerRight), Some(Pad::R2));
        assert_eq!(trigger_of(Axis::LeftX), None);
        assert_eq!(pad_of(Button::LeftStick), Some(Pad::L3));
        assert_eq!(pad_of(Button::RightStick), Some(Pad::R3));
    }

    #[test]
    fn a_key_event_round_trips_to_the_name_that_resolves_it() {
        let names = KeyNames::new();
        for keycode in [
            Keycode::RETURN,
            Keycode::PAGEDOWN,
            Keycode::LCTRL,
            Keycode::A,
        ] {
            let name = key_name(keycode);
            assert_eq!(
                names.code(&name),
                Some(key_code(keycode)),
                "round trip for `{name}`"
            );
        }
    }
}
