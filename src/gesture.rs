//! Gesture text: the spellings a bindings file uses, and their values.
//!
//! Pad gestures are `a` (tap), `hold:y`, `select+x` (chord, in press order).
//! Key gestures are modifier combos over a key name, `ctrl+shift+t`. Names are
//! normalized on the way in — lowercased with spaces, underscores and hyphens
//! dropped — so a file may spell SDL's own `Page Down` as `pagedown`.

use super::pad::Pad;
use super::Mods;

/// A gesture on the pad.
///
/// A chord is ordered — `select+x` means hold Select, then press X — so only the
/// leader is ambiguous on its press edge and has to wait. The second pad either
/// completes a chord or is a plain tap, decidable at once, which is what lets a
/// paging pad take part in one at all.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PadGesture {
    Tap(Pad),
    Hold(Pad),
    /// Leader first, then the pad that completes it.
    Chord(Pad, Pad),
}

impl PadGesture {
    pub fn parse(text: &str) -> Option<Self> {
        let text = normalize(text);
        if let Some(rest) = text.strip_prefix("hold:") {
            return Pad::parse(rest).map(PadGesture::Hold);
        }
        if let Some((first, second)) = text.split_once('+') {
            let (a, b) = (Pad::parse(first)?, Pad::parse(second)?);
            return (a != b).then(|| PadGesture::chord(a, b));
        }
        Pad::parse(&text).map(PadGesture::Tap)
    }

    /// A chord in press order: `leader` was already down when `second` arrived.
    pub fn chord(leader: Pad, second: Pad) -> Self {
        PadGesture::Chord(leader, second)
    }

    pub fn to_text(self) -> String {
        match self {
            PadGesture::Tap(pad) => pad.name().to_string(),
            PadGesture::Hold(pad) => format!("hold:{}", pad.name()),
            PadGesture::Chord(a, b) => format!("{}+{}", a.name(), b.name()),
        }
    }
}

/// A key with its modifiers. The name stays text here: which names exist is the
/// backend's to know, and it resolves them to codes when the table is built.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyGesture {
    pub name: String,
    pub mods: Mods,
}

impl KeyGesture {
    /// Parse `ctrl+shift+t`. A literal `+` cannot be bound — it separates the
    /// tokens — and neither can a combo naming no key at all.
    pub fn parse(text: &str) -> Option<Self> {
        let mut mods = Mods::NONE;
        let mut name = None;
        for token in normalize(text).split('+') {
            match token {
                "ctrl" | "control" => mods = mods.union(Mods::CTRL),
                "alt" => mods = mods.union(Mods::ALT),
                "shift" => mods = mods.union(Mods::SHIFT),
                "" => return None,
                token if name.is_none() => name = Some(token.to_string()),
                _ => return None, // two key names in one gesture
            }
        }
        Some(Self { name: name?, mods })
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (mask, label) in [
            (Mods::CTRL, "ctrl+"),
            (Mods::ALT, "alt+"),
            (Mods::SHIFT, "shift+"),
        ] {
            if self.mods.contains(mask) {
                out.push_str(label);
            }
        }
        out.push_str(&self.name);
        out
    }
}

/// Lowercase and drop the spacing SDL's own key names carry, so `Page Down`,
/// `page_down` and `pagedown` are one name. `-` survives: it is a key in its
/// own right.
pub(super) fn normalize(text: &str) -> String {
    text.trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_gestures_round_trip() {
        for text in ["a", "hold:y", "l1+r1"] {
            let gesture = PadGesture::parse(text).expect("valid gesture");
            assert_eq!(gesture.to_text(), text);
        }
    }

    #[test]
    fn a_chord_keeps_its_press_order() {
        // Order is the gesture: which pad leads decides which one waits.
        assert_eq!(
            PadGesture::parse("select+x"),
            Some(PadGesture::Chord(Pad::Select, Pad::X))
        );
        assert_ne!(PadGesture::parse("x+select"), PadGesture::parse("select+x"));
        assert_eq!(
            PadGesture::parse("x+select")
                .map(PadGesture::to_text)
                .as_deref(),
            Some("x+select")
        );
        // A pad chorded with itself is not a gesture.
        assert_eq!(PadGesture::parse("l1+l1"), None);
    }

    #[test]
    fn unknown_pads_and_stray_text_are_rejected() {
        assert_eq!(PadGesture::parse("l9"), None);
        assert_eq!(PadGesture::parse("hold:elbow"), None);
        assert_eq!(PadGesture::parse("a+"), None);
    }

    #[test]
    fn key_names_normalize_but_modifiers_stay_ordered() {
        let g = KeyGesture::parse("Shift+Ctrl+Page Down").expect("valid gesture");
        assert_eq!(g.name, "pagedown");
        assert!(g.mods.contains(Mods::CTRL) && g.mods.contains(Mods::SHIFT));
        assert!(!g.mods.contains(Mods::ALT));
        // Written back, modifiers take a fixed order.
        assert_eq!(g.to_text(), "ctrl+shift+pagedown");
    }

    #[test]
    fn a_gesture_naming_no_key_or_two_is_rejected() {
        assert_eq!(KeyGesture::parse("ctrl"), None);
        assert_eq!(KeyGesture::parse("ctrl+a+b"), None);
        assert_eq!(KeyGesture::parse("ctrl++"), None);
    }
}
