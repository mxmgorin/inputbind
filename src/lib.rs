//! Rebindable input: gestures on a pad or a keyboard resolve to the host app's
//! own action type. Two modules name backends: [`sdl`], behind the `sdl2`
//! feature, and [`evdev`], the keypad read from the Linux kernel for devices
//! whose own input layer is lossy.
//!
//! The app supplies three things: an [`Action`] vocabulary, the default
//! [`Store`], and a key-name resolver (the backend knows which names exist).
//! Everything else — gesture text, gesture resolution, the file, the editor
//! model — lives here.
//!
//! Runtime tables are built once and hold no strings: pads index a fixed array,
//! the deferred set is a bitmask, chords are a packed array, and key names are
//! resolved to numeric codes at load. The input path never allocates.

mod capture;
pub mod editor;
pub mod evdev;
mod gesture;
mod keys;
mod pad;
mod repeat;
#[cfg(feature = "sdl2")]
pub mod sdl;
mod store;

pub use capture::{Capture, Captured, Tick};
pub use gesture::{KeyGesture, PadGesture};
pub use keys::KeyState;
pub use pad::{Edge, Pad, PadState, Stick, Trigger};
pub use repeat::Cadence;
pub use store::{Store, Table, UNBOUND};

/// What a gesture resolves to. The host app owns this vocabulary; this module
/// stores, looks up and edits it without knowing what any of it means.
pub trait Action: Copy + Eq + 'static {
    /// Spelling in the bindings file; [`Action::parse`] is its inverse, so no
    /// caller has to spell a raw string.
    fn name(&self) -> &'static str;

    fn parse(name: &str) -> Option<Self>;

    /// Every bindable action. Display order is the caller's to choose.
    fn all() -> &'static [Self];

    /// Label for the editor; defaults to the config spelling.
    fn display(&self) -> &'static str {
        self.name()
    }

    /// Auto-repeats while its input is held — navigation, not a one-shot.
    fn repeats(&self) -> bool {
        false
    }

    /// Spans press and release, as a click does. Resolved by a release, where no
    /// edge is left to wait for, both go at once.
    fn is_held(&self) -> bool {
        false
    }

    /// Must fire on the press edge, so `hold:` and chord gestures are refused
    /// on any pad whose tap carries it: deferring the tap to release would cost
    /// exactly the timing such an action depends on.
    fn needs_press_edge(&self) -> bool {
        false
    }
}

/// Modifier mask for a key gesture; left and right variants fold together.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Mods = Mods(0);
    pub const CTRL: Mods = Mods(1);
    pub const ALT: Mods = Mods(2);
    pub const SHIFT: Mods = Mods(4);

    /// Neither Ctrl nor Alt, so it collides with typing; Shift alone is still plain.
    pub fn is_plain(self) -> bool {
        !self.contains(Mods::CTRL) && !self.contains(Mods::ALT)
    }

    pub fn contains(self, other: Mods) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn union(self, other: Mods) -> Mods {
        Mods(self.0 | other.0)
    }

    pub fn without(self, other: Mods) -> Mods {
        Mods(self.0 & !other.0)
    }
}

/// A surface with its own override table, resolved once from its name so the
/// input path indexes rather than compares strings.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct SurfaceId(u8);

/// One override cell.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Override<A> {
    /// No entry: the base table decides.
    Fallthrough,
    /// Bound to `none`: nothing happens, and the base binding stays suppressed.
    Unbound,
    Bound(A),
}

/// The pad tables, built once and read-only after.
struct PadTable<A> {
    tap: [Option<A>; Pad::COUNT],
    hold: [Option<A>; Pad::COUNT],
    /// Keyed by the ordered pair `(leader, second)` — few enough that a scan
    /// beats hashing.
    chords: Vec<((Pad, Pad), A)>,
    /// Pads whose tap must wait for release, because a hold or leading a chord
    /// makes the press edge ambiguous. Global on purpose: an override may change
    /// what a gesture *means* per surface, never when it fires.
    deferred: u16,
}

pub struct Bindings<A: Action> {
    base: PadTable<A>,
    surfaces: Vec<(&'static str, [Override<A>; Pad::COUNT])>,
    /// Sorted by (code, mods) for binary search; modifiers must match exactly.
    keys: Vec<((u32, Mods), A)>,
}

impl<A: Action> Bindings<A> {
    /// Parse a [`Store`] into the runtime tables. Entries that do not parse are
    /// logged and skipped: a hand-edited file is the user's, and one bad line
    /// must not cost them the rest. `resolve_key` maps a normalized key name to
    /// the backend's code.
    pub fn new(
        store: &Store,
        surfaces: &[&'static str],
        resolve_key: impl Fn(&str) -> Option<u32>,
    ) -> Self {
        let mut base = PadTable {
            tap: [None; Pad::COUNT],
            hold: [None; Pad::COUNT],
            chords: Vec::new(),
            deferred: 0,
        };

        for (text, name) in &store.gamepad {
            let Some(gesture) = PadGesture::parse(text) else {
                log::warn!("bindings: unknown gesture `{text}`");
                continue;
            };
            // `none` in the base table is the same as saying nothing.
            if name == UNBOUND {
                continue;
            }
            let Some(action) = A::parse(name) else {
                log::warn!("bindings: unknown action `{name}` for `{text}`");
                continue;
            };
            match gesture {
                PadGesture::Tap(pad) => base.tap[pad as usize] = Some(action),
                PadGesture::Hold(pad) => base.hold[pad as usize] = Some(action),
                PadGesture::Chord(leader, second) => base.chords.push(((leader, second), action)),
            }
        }

        base.drop_gestures_that_would_defer_a_press_edge();
        base.deferred = base.deferred_pads();

        let surface_tables: Vec<(&'static str, [Override<A>; Pad::COUNT])> = surfaces
            .iter()
            .map(|name| {
                let mut table = [Override::Fallthrough; Pad::COUNT];
                if let Some(entries) = store.surface.get(*name) {
                    for (text, action) in entries {
                        match PadGesture::parse(text) {
                            // Overrides remap a tap only: holds and chords are
                            // bound to actions that mean the same everywhere,
                            // and changing the gesture set per surface would
                            // move the press edge with it.
                            Some(PadGesture::Tap(pad)) => {
                                table[pad as usize] = if action == UNBOUND {
                                    Override::Unbound
                                } else {
                                    match A::parse(action) {
                                        Some(a) => Override::Bound(a),
                                        None => {
                                            log::warn!(
                                                "bindings: unknown action `{action}` for `{text}` in [surface.{name}]"
                                            );
                                            continue;
                                        }
                                    }
                                };
                            }
                            _ => log::warn!(
                                "bindings: [surface.{name}] takes taps only; ignoring `{text}`"
                            ),
                        }
                    }
                }
                (*name, table)
            })
            .collect();

        for name in store.surface.keys() {
            if !surfaces.contains(&name.as_str()) {
                log::warn!("bindings: unknown surface `{name}`; ignored");
            }
        }

        let mut keys: Vec<((u32, Mods), A)> = Vec::new();
        for (text, name) in &store.keyboard {
            // A key has no gesture machine, so `hold:`/chord text here is not a
            // typo to report as an unknown key — it is a thing keys cannot do.
            if text.contains("hold:") || text.contains('+') && KeyGesture::parse(text).is_none() {
                log::warn!("bindings: `{text}` — a key takes no hold or chord, only modifiers");
                continue;
            }
            let Some(gesture) = KeyGesture::parse(text) else {
                log::warn!("bindings: unknown key gesture `{text}`");
                continue;
            };
            if name == UNBOUND {
                continue;
            }
            let Some(action) = A::parse(name) else {
                log::warn!("bindings: unknown action `{name}` for `{text}`");
                continue;
            };
            let Some(code) = resolve_key(&gesture.name) else {
                log::warn!("bindings: unknown key `{}` in `{text}`", gesture.name);
                continue;
            };
            keys.push(((code, gesture.mods), action));
        }
        keys.sort_by_key(|((code, mods), _)| (*code, *mods));
        keys.dedup_by_key(|((code, mods), _)| (*code, *mods));

        Self {
            base,
            surfaces: surface_tables,
            keys,
        }
    }

    /// The id for a surface's override table, resolved once by the app.
    pub fn surface_id(&self, name: &str) -> Option<SurfaceId> {
        self.surfaces
            .iter()
            .position(|(n, _)| *n == name)
            .map(|i| SurfaceId(i as u8))
    }

    /// The tap action for `pad`, letting `surface`'s override win.
    pub fn tap(&self, pad: Pad, surface: Option<SurfaceId>) -> Option<A> {
        if let Some(SurfaceId(i)) = surface {
            match self.surfaces[i as usize].1[pad as usize] {
                Override::Bound(action) => return Some(action),
                Override::Unbound => return None,
                Override::Fallthrough => {}
            }
        }
        self.base.tap[pad as usize]
    }

    pub fn hold(&self, pad: Pad) -> Option<A> {
        self.base.hold[pad as usize]
    }

    /// The chord completed by pressing `second` while `leader` is held. Ordered:
    /// pressing them the other way round is a different gesture, or none.
    pub fn chord(&self, leader: Pad, second: Pad) -> Option<A> {
        self.base
            .chords
            .iter()
            .find(|((l, s), _)| *l == leader && *s == second)
            .map(|(_, action)| *action)
    }

    /// Whether this pad's tap waits for release.
    pub fn is_deferred(&self, pad: Pad) -> bool {
        self.base.deferred & pad.bit() != 0
    }

    /// The pad that would refuse `gesture`, because its tap must fire on the
    /// press edge and this gesture would defer it. Ask before binding: the
    /// alternative is writing a gesture the tables then drop.
    pub fn press_edge_conflict(&self, gesture: PadGesture) -> Option<Pad> {
        let refuses = |pad: Pad| {
            self.base.tap[pad as usize]
                .is_some_and(|a| a.needs_press_edge())
                .then_some(pad)
        };
        match gesture {
            // A tap replaces a tap; nothing gets deferred.
            PadGesture::Tap(_) => None,
            PadGesture::Hold(pad) => refuses(pad),
            // Only the leader waits; the pad completing it keeps its press edge.
            PadGesture::Chord(leader, _) => refuses(leader),
        }
    }

    /// The action for a key press. Modifiers must match exactly, so a gesture
    /// without Shift will not fire while Shift is down.
    pub fn key(&self, code: u32, mods: Mods) -> Option<A> {
        self.keys
            .binary_search_by_key(&(code, mods), |(key, _)| *key)
            .ok()
            .map(|i| self.keys[i].1)
    }
}

impl<A: Action> PadTable<A> {
    /// A hold or chord defers its pads' taps to release. Where that tap must
    /// fire on the press edge — paging, auto-repeat — the extra gesture loses,
    /// since the alternative is silently degrading the frequent action.
    fn drop_gestures_that_would_defer_a_press_edge(&mut self) {
        let mut press_edge = 0u16;
        for pad in Pad::ALL {
            if self.tap[pad as usize].is_some_and(|a| a.needs_press_edge()) {
                press_edge |= pad.bit();
            }
        }
        for pad in Pad::ALL {
            if self.hold[pad as usize].is_some() && press_edge & pad.bit() != 0 {
                log::warn!(
                    "bindings: ignoring hold:{} — its tap must fire on the press edge",
                    pad.name()
                );
                self.hold[pad as usize] = None;
            }
        }
        self.chords.retain(|((leader, second), _)| {
            if press_edge & leader.bit() == 0 {
                return true;
            }
            log::warn!(
                "bindings: ignoring the chord {}+{} — {} leads it, and its tap must fire on the press edge",
                leader.name(),
                second.name(),
                leader.name()
            );
            false
        });
    }

    fn deferred_pads(&self) -> u16 {
        let mut deferred = 0;
        for pad in Pad::ALL {
            if self.hold[pad as usize].is_some() {
                deferred |= pad.bit();
            }
        }
        // Leading a chord is what defers a pad: the second one resolves on its
        // own press edge, so it stays immediate.
        for ((leader, _), _) in &self.chords {
            deferred |= leader.bit();
        }
        deferred
    }
}

/// Shared test fixture: a small action vocabulary, a bindings builder, and an
/// instant-as-offset helper for the time-driven tests.
#[cfg(test)]
mod testkit {
    use super::*;
    use std::time::{Duration, Instant};

    #[derive(Copy, Clone, PartialEq, Eq, Debug)]
    pub(super) enum TestAction {
        Confirm,
        PageNext,
        ThemeNext,
        NavDown,
        /// Spans both edges, as a host's click does.
        Click,
    }

    impl Action for TestAction {
        fn name(&self) -> &'static str {
            match self {
                TestAction::Confirm => "confirm",
                TestAction::PageNext => "page_next",
                TestAction::ThemeNext => "theme_next",
                TestAction::NavDown => "nav_down",
                TestAction::Click => "click",
            }
        }

        fn parse(name: &str) -> Option<Self> {
            Self::all().iter().copied().find(|a| a.name() == name)
        }

        fn all() -> &'static [Self] {
            &[
                TestAction::Confirm,
                TestAction::PageNext,
                TestAction::ThemeNext,
                TestAction::NavDown,
                TestAction::Click,
            ]
        }

        // As in the host app: what repeats is exactly what needs the press edge.
        fn repeats(&self) -> bool {
            matches!(self, TestAction::NavDown | TestAction::PageNext)
        }

        fn is_held(&self) -> bool {
            matches!(self, TestAction::Click)
        }

        fn needs_press_edge(&self) -> bool {
            self.repeats()
        }
    }

    const SURFACES: [&str; 1] = ["reader"];

    pub(super) fn build(toml_text: &str) -> Bindings<TestAction> {
        let store: Store = toml::from_str(toml_text).expect("valid test store");
        // A fake resolver: the key name's first byte stands in for a keycode.
        Bindings::new(&store, &SURFACES, |name| name.bytes().next().map(u32::from))
    }

    /// `t0` shifted `ms` forward, so tests spell instants as offsets.
    pub(super) fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    /// Just the actions, for assertions that do not care about edges.
    pub(super) fn actions<A: Copy>(out: &[(A, Edge)]) -> Vec<A> {
        out.iter().map(|(action, _)| *action).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{build, TestAction};
    use super::*;

    #[test]
    fn a_tap_only_pad_is_not_deferred() {
        let b = build("[gamepad]\na = \"confirm\"\n");
        assert_eq!(b.tap(Pad::A, None), Some(TestAction::Confirm));
        assert!(!b.is_deferred(Pad::A));
    }

    #[test]
    fn a_hold_defers_its_pad() {
        let b = build("[gamepad]\ny = \"confirm\"\n\"hold:y\" = \"theme_next\"\n");
        assert!(b.is_deferred(Pad::Y));
        assert_eq!(b.hold(Pad::Y), Some(TestAction::ThemeNext));
    }

    #[test]
    fn a_chord_defers_only_its_leader() {
        let b = build("[gamepad]\n\"start+select\" = \"theme_next\"\n");
        assert!(b.is_deferred(Pad::Start), "the leader waits");
        assert!(
            !b.is_deferred(Pad::Select),
            "the second pad resolves on its own press edge"
        );
        // Ordered: only Start-then-Select is this gesture.
        assert_eq!(
            b.chord(Pad::Start, Pad::Select),
            Some(TestAction::ThemeNext)
        );
        assert_eq!(b.chord(Pad::Select, Pad::Start), None);
    }

    #[test]
    fn a_paging_pad_can_complete_a_chord_it_does_not_lead() {
        // The point of ordering: page_next keeps its press edge and still takes
        // part, where the unordered form had to refuse the whole chord.
        let b = build(
            "[gamepad]\nx = \"page_next\"\nselect = \"confirm\"\n\"select+x\" = \"theme_next\"\n",
        );
        assert_eq!(b.chord(Pad::Select, Pad::X), Some(TestAction::ThemeNext));
        assert!(
            !b.is_deferred(Pad::X),
            "X must still page on its press edge"
        );
        assert!(b.is_deferred(Pad::Select));

        // Led by the paging pad, it is still refused.
        let b = build(
            "[gamepad]\nx = \"page_next\"\nselect = \"confirm\"\n\"x+select\" = \"theme_next\"\n",
        );
        assert_eq!(b.chord(Pad::X, Pad::Select), None);
        assert!(!b.is_deferred(Pad::X));
    }

    #[test]
    fn a_press_edge_tap_refuses_the_gestures_that_would_defer_it() {
        // Paging and navigation need the press edge, so the extra gesture loses
        // rather than quietly moving the page turn to release.
        let b = build("[gamepad]\nr1 = \"page_next\"\n\"hold:r1\" = \"theme_next\"\n");
        assert_eq!(b.hold(Pad::R1), None);
        assert!(!b.is_deferred(Pad::R1));

        let b = build("[gamepad]\ndown = \"nav_down\"\n\"down+start\" = \"theme_next\"\n");
        assert_eq!(b.chord(Pad::Down, Pad::Start), None);
        assert!(!b.is_deferred(Pad::Down) && !b.is_deferred(Pad::Start));
    }

    #[test]
    fn a_surface_override_replaces_or_suppresses_the_base_tap() {
        let b = build(
            "[gamepad]\na = \"confirm\"\nb = \"confirm\"\n\
             [surface.reader]\na = \"none\"\nb = \"page_next\"\n",
        );
        let reader = b.surface_id("reader");
        assert!(reader.is_some());
        // Off that surface the base still stands.
        assert_eq!(b.tap(Pad::A, None), Some(TestAction::Confirm));
        assert_eq!(b.tap(Pad::A, reader), None);
        assert_eq!(b.tap(Pad::B, reader), Some(TestAction::PageNext));
    }

    #[test]
    fn an_override_does_not_move_the_press_edge() {
        // The base tap needs the press edge, so the hold is refused — and an
        // override naming a different action must not bring it back.
        let b = build(
            "[gamepad]\nr1 = \"page_next\"\n\"hold:r1\" = \"theme_next\"\n\
             [surface.reader]\nr1 = \"confirm\"\n",
        );
        assert!(!b.is_deferred(Pad::R1));
        assert_eq!(b.hold(Pad::R1), None);
    }

    #[test]
    fn keys_match_their_modifiers_exactly() {
        let b = build("[keyboard]\n\"ctrl+r\" = \"theme_next\"\nf = \"confirm\"\n");
        let r = u32::from(b'r');
        let f = u32::from(b'f');
        assert_eq!(b.key(r, Mods::CTRL), Some(TestAction::ThemeNext));
        assert_eq!(b.key(r, Mods::NONE), None);
        assert_eq!(b.key(f, Mods::NONE), Some(TestAction::Confirm));
        assert_eq!(b.key(f, Mods::SHIFT), None);
    }

    #[test]
    fn a_bad_entry_is_skipped_and_the_rest_survives() {
        let b = build("[gamepad]\na = \"confirm\"\nl9 = \"confirm\"\nx = \"fly\"\nb = \"none\"\n");
        assert_eq!(b.tap(Pad::A, None), Some(TestAction::Confirm));
        assert_eq!(b.tap(Pad::X, None), None);
        // `none` in the base table binds nothing at all.
        assert_eq!(b.tap(Pad::B, None), None);
    }

    #[test]
    fn a_gesture_that_would_defer_a_press_edge_tap_is_reported_before_binding() {
        let b = build("[gamepad]\nr1 = \"page_next\"\na = \"confirm\"\n");
        // r1 pages, so a hold or chord there would move the page turn to release.
        assert_eq!(
            b.press_edge_conflict(PadGesture::Hold(Pad::R1)),
            Some(Pad::R1)
        );
        assert_eq!(
            b.press_edge_conflict(PadGesture::chord(Pad::R1, Pad::Start)),
            Some(Pad::R1)
        );
        // The other way round R1 only completes the chord, so it is fine.
        assert_eq!(
            b.press_edge_conflict(PadGesture::chord(Pad::Start, Pad::R1)),
            None
        );
        // A pad whose tap is not rhythmic takes either happily.
        assert_eq!(b.press_edge_conflict(PadGesture::Hold(Pad::A)), None);
        assert_eq!(
            b.press_edge_conflict(PadGesture::chord(Pad::A, Pad::Start)),
            None
        );
        // A tap replaces a tap, so it never defers anything.
        assert_eq!(b.press_edge_conflict(PadGesture::Tap(Pad::R1)), None);
    }

    #[test]
    fn a_key_gesture_cannot_hold_or_chord() {
        // Keys have no gesture machine; such an entry is skipped, not guessed at.
        let b = build("[keyboard]\n\"hold:x\" = \"confirm\"\nx = \"confirm\"\n");
        let x = u32::from(b'x');
        assert_eq!(b.key(x, Mods::NONE), Some(TestAction::Confirm));
    }
}
