//! Devices whose pad arrives as keys.
//!
//! Some SDL2 ports offer no controller mapping and send keys instead — the
//! Miyoo Mini is one — so those keys *are* the pad: fed to the pad machine,
//! they get gestures, and the `[gamepad]` table is the one they answer to. A
//! desktop keyboard is left alone — there, keys are keys and bind straight to
//! actions.

use crate::Pad;
use sdl2::keyboard::Keycode;

/// Which layout the keys arriving from the device follow.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Keymap {
    /// Keys are keys.
    #[default]
    Desktop,
    /// Keys are the pad.
    MiyooMini,
}

impl Keymap {
    /// The host's explicit request (`miyoo`|`desktop`, e.g. an env override)
    /// wins; else the video driver gives the device away.
    pub fn detect(video_driver: &str, requested: Option<&str>) -> Self {
        match requested {
            Some("miyoo") => Self::MiyooMini,
            Some("desktop") => Self::Desktop,
            Some(other) => {
                log::warn!("unknown keymap `{other}`; using the desktop layout");
                Self::Desktop
            }
            None if video_driver == "mmiyoo" => Self::MiyooMini,
            None => Self::Desktop,
        }
    }

    /// The pad input this key stands for, or `None` where keys are keys.
    pub fn pad(self, keycode: Keycode) -> Option<Pad> {
        match self {
            Keymap::Desktop => None,
            Keymap::MiyooMini => MIYOO
                .iter()
                .find(|(key, _)| *key == keycode)
                .map(|(_, pad)| *pad),
        }
    }
}

/// The Miyoo pad, as the keys it sends. MENU is absent: the launcher gives it to
/// the system's own kill helper.
const MIYOO: &[(Keycode, Pad)] = &[
    (Keycode::SPACE, Pad::A),
    (Keycode::LCTRL, Pad::B),
    (Keycode::LSHIFT, Pad::X),
    (Keycode::LALT, Pad::Y),
    (Keycode::RETURN, Pad::Start),
    (Keycode::RCTRL, Pad::Select),
    (Keycode::E, Pad::L1),
    (Keycode::T, Pad::R1),
    // The device sends its own shoulders, but a USB keyboard in the dock sends
    // these.
    (Keycode::PAGEUP, Pad::L1),
    (Keycode::PAGEDOWN, Pad::R1),
    (Keycode::UP, Pad::Up),
    (Keycode::DOWN, Pad::Down),
    (Keycode::LEFT, Pad::Left),
    (Keycode::RIGHT, Pad::Right),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_prefers_the_request_over_the_driver() {
        assert_eq!(Keymap::detect("x11", Some("miyoo")), Keymap::MiyooMini);
        assert_eq!(Keymap::detect("mmiyoo", Some("desktop")), Keymap::Desktop);
        assert_eq!(Keymap::detect("mmiyoo", None), Keymap::MiyooMini);
        assert_eq!(Keymap::detect("x11", None), Keymap::Desktop);
        // An unknown request falls back rather than guessing.
        assert_eq!(Keymap::detect("mmiyoo", Some("elbow")), Keymap::Desktop);
    }

    #[test]
    fn a_desktop_key_is_never_a_pad() {
        for key in [Keycode::X, Keycode::Z, Keycode::RETURN, Keycode::UP] {
            assert_eq!(Keymap::Desktop.pad(key), None);
        }
    }

    #[test]
    fn the_miyoo_keys_are_its_pad() {
        for (key, pad) in [
            (Keycode::SPACE, Pad::A),
            (Keycode::LCTRL, Pad::B),
            (Keycode::LSHIFT, Pad::X),
            (Keycode::LALT, Pad::Y),
            (Keycode::RETURN, Pad::Start),
            (Keycode::RCTRL, Pad::Select),
            (Keycode::E, Pad::L1),
            (Keycode::T, Pad::R1),
            (Keycode::DOWN, Pad::Down),
        ] {
            assert_eq!(Keymap::MiyooMini.pad(key), Some(pad));
        }
    }

    #[test]
    fn a_key_the_miyoo_pad_does_not_use_stays_a_key() {
        // Nothing to translate, so it falls through to the `[keyboard]` table.
        assert_eq!(Keymap::MiyooMini.pad(Keycode::G), None);
        assert_eq!(Keymap::MiyooMini.pad(Keycode::EQUALS), None);
    }

    #[test]
    fn no_key_claims_two_pads() {
        let mut keys: Vec<i32> = MIYOO.iter().map(|(k, _)| i32::from(*k)).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "a key maps to two pads");
    }
}
