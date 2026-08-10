//! The bindings editor model: every action with the gestures bound to it, and
//! a row per edit. Pure state — the host draws the rows, runs capture, and
//! applies the edits to the [`Store`]; rebuilding after each edit keeps the
//! screen and the input path from ever disagreeing.
//!
//! Rows are built from the store when the screen opens and after each edit, not
//! per frame: the list is long and a renderer runs on every pass.

use crate::{Action, Pad, PadGesture, Store, Table, UNBOUND};

/// Which table a gesture lives in.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Source {
    Gamepad,
    Keyboard,
    /// A `[surface.*]` override, by surface name.
    Surface(&'static str),
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Gamepad => "pad",
            Source::Keyboard => "key",
            Source::Surface(name) => name,
        }
    }
}

/// A gesture as a collapsed [`Row::Command`] lists it.
#[derive(Clone, Debug)]
pub struct Bound {
    pub text: String,
    pub source: Source,
    /// Bound in the base table but switched off on a surface — listed so the
    /// line cannot show a binding while hiding that it does nothing there.
    pub suppressed: bool,
}

#[derive(Clone, Debug)]
pub enum Row<A> {
    /// A section's name. Not selectable.
    Group(&'static str),
    /// An action and everything bound to it, on one line. Activating it opens
    /// the rows below for editing, and closes them again.
    Command {
        action: A,
        gestures: Vec<Bound>,
        open: bool,
    },
    /// A gesture of the open command; activating it unbinds. Only ever listed
    /// under an open one.
    Gesture { text: String, source: Source },
    /// One of its gestures that a `none` override switched off on a surface.
    Suppressed { text: String, surface: &'static str },
    /// Activating starts listening for a gesture to bind to this action.
    Add(A),
}

impl<A> Row<A> {
    /// A group labels the commands under it; only the rest can be focused,
    /// tapped or activated.
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Group(_))
    }
}

/// The editor's sections: a label and the actions under it. Sections come in
/// declared order; actions sort by display name within each.
pub type Groups<A> = &'static [(&'static str, &'static [A])];

/// A requirement on a table: its label, for the message explaining a refusal,
/// and the actions any one of which satisfies it.
pub type Requirement<A> = (&'static str, &'static [A]);

pub struct Controls<A: Action> {
    pub open: bool,
    rows: Vec<Row<A>>,
    /// Indices of the selectable rows, so cursor moves skip headers.
    selectable: Vec<usize>,
    cursor: usize,
    /// The action being bound, while capture listens.
    capturing: Option<A>,
    /// The one command showing its gestures. One at a time keeps the list the
    /// length of the command set, which is the point of collapsing it.
    open_command: Option<A>,
    groups: Groups<A>,
    surfaces: &'static [&'static str],
    /// Rows the host draws after these, which the cursor also spans.
    trailing: usize,
}

impl<A: Action> Controls<A> {
    /// `trailing` selectable rows follow these, indexed from `rows().len()`, so
    /// the host needs no second cursor of its own.
    pub fn new(groups: Groups<A>, surfaces: &'static [&'static str], trailing: usize) -> Self {
        Self {
            open: false,
            rows: Vec::new(),
            selectable: Vec::new(),
            cursor: 0,
            capturing: None,
            open_command: None,
            groups,
            surfaces,
            trailing,
        }
    }

    /// Opens collapsed: the list is the command set, and a screen kept open
    /// from last time would be someone else's place, not yours.
    pub fn show(&mut self, store: &Store) {
        self.open_command = None;
        self.rebuild(store);
        self.focus_first();
        self.open = true;
    }

    pub fn focus_first(&mut self) {
        self.cursor = self.selectable.first().copied().unwrap_or(0);
    }

    /// Which trailing row the cursor is on; [`Self::selected`] is `None` for these.
    pub fn trailing_cursor(&self) -> Option<usize> {
        self.cursor.checked_sub(self.rows.len())
    }

    pub fn close(&mut self) {
        self.open = false;
        self.capturing = None;
    }

    /// Open a command's gestures, or close them if they are the open ones.
    pub fn toggle_command(&mut self, action: A, store: &Store) {
        self.open_command = if self.open_command == Some(action) {
            None
        } else {
            Some(action)
        };
        self.rebuild(store);
    }

    pub fn rows(&self) -> &[Row<A>] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn capturing(&self) -> Option<A> {
        self.capturing
    }

    pub fn start_capture(&mut self, action: A) {
        self.capturing = Some(action);
    }

    pub fn stop_capture(&mut self) {
        self.capturing = None;
    }

    pub fn selected(&self) -> Option<&Row<A>> {
        self.rows.get(self.cursor)
    }

    /// Step the cursor over selectable rows only, wrapping at the ends: the
    /// host's own rows sit last, and paging the whole list is a poor way to
    /// reach them.
    pub fn move_cursor(&mut self, delta: i32) {
        if self.selectable.is_empty() {
            return;
        }
        let at = self
            .selectable
            .iter()
            .position(|i| *i == self.cursor)
            .unwrap_or(0) as i32;
        let len = self.selectable.len() as i32;
        self.cursor = self.selectable[(at + delta).rem_euclid(len) as usize];
    }

    /// A tap lands on a row directly; a header tap is ignored.
    pub fn set_cursor(&mut self, index: usize) {
        if self.selectable.contains(&index) {
            self.cursor = index;
        }
    }

    /// Rebuild from the store, keeping the cursor on a real row: an edit changes
    /// how many rows an action has.
    pub fn rebuild(&mut self, store: &Store) {
        // A cursor on one of the host's rows keeps that row rather than its
        // index: opening a command changes how many rows precede it.
        let trailing = self.trailing_cursor().filter(|k| *k < self.trailing);
        self.rows = build_rows(self.groups, self.surfaces, store, self.open_command);
        self.selectable = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.selectable())
            .map(|(i, _)| i)
            .chain(self.rows.len()..self.rows.len() + self.trailing)
            .collect();
        if let Some(k) = trailing {
            self.cursor = self.rows.len() + k;
            return;
        }
        if self.selectable.contains(&self.cursor) {
            return;
        }
        // Nearest selectable at or before where it was, so removing the last
        // gesture of an action leaves you on that action's Add row.
        self.cursor = self
            .selectable
            .iter()
            .rev()
            .find(|i| **i <= self.cursor)
            .or_else(|| self.selectable.first())
            .copied()
            .unwrap_or(0);
    }
}

/// Why an edit cannot be made; the host words it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// Its tap must fire on the press edge, which this gesture would defer.
    PressEdge(Pad),
    /// The requirement's label; the table would stop meeting it.
    Requirement(&'static str),
}

/// Whether `table` can take `gesture`, bound to `becomes` or unbound with `None`.
/// Ask before editing, or the tables drop the gesture at load and say nothing.
/// A `[keyboard]` table has no press edge, so it gets the requirement check only.
pub fn validate<A: Action>(
    table: &Table,
    gesture: &str,
    becomes: Option<A>,
    required: &[Requirement<A>],
) -> Result<(), Refusal> {
    if becomes.is_some() {
        if let Some(pad) = press_edge_conflict::<A>(table, gesture) {
            return Err(Refusal::PressEdge(pad));
        }
    }
    match requirement_lost(table, gesture, becomes, required) {
        Some(label) => Err(Refusal::Requirement(label)),
        None => Ok(()),
    }
}

/// The [`Table`] counterpart of
/// [`Bindings::press_edge_conflict`](crate::Bindings::press_edge_conflict); a test
/// holds the two to the same answer.
fn press_edge_conflict<A: Action>(table: &Table, gesture: &str) -> Option<Pad> {
    // Only the leader waits; a tap replaces a tap, and the pad completing a
    // chord keeps its own press edge.
    let leader = match PadGesture::parse(gesture)? {
        PadGesture::Tap(_) => return None,
        PadGesture::Hold(pad) => pad,
        PadGesture::Chord(leader, _) => leader,
    };
    let bound = table.get(leader.name()).and_then(|name| A::parse(name))?;
    bound.needs_press_edge().then_some(leader)
}

/// The requirement `table` would no longer meet once `text` stops naming what it
/// names now — unbound (`becomes` is `None`) or rebound to something else.
/// Returns its label, for the message explaining the refusal. Ask before
/// editing: the alternative is a table the host cannot be operated from.
pub fn requirement_lost<A: Action>(
    table: &Table,
    text: &str,
    becomes: Option<A>,
    required: &[Requirement<A>],
) -> Option<&'static str> {
    required
        .iter()
        .find(|(_, actions)| {
            let met = actions.iter().any(|action| {
                // The edited gesture counts only if the edit leaves it naming
                // this action; any *other* gesture counts as it stands.
                becomes == Some(*action)
                    || table
                        .iter()
                        .any(|(g, bound)| g != text && bound == action.name())
            });
            !met
        })
        .map(|(label, _)| *label)
}

/// Whether a table meets every requirement as it stands.
pub fn meets_every_requirement<A: Action>(table: &Table, required: &[Requirement<A>]) -> bool {
    required.iter().all(|(_, actions)| {
        actions
            .iter()
            .any(|action| table.values().any(|bound| bound == action.name()))
    })
}

/// A line per action, carrying what is bound to it, and the edit rows of the one
/// that is open. Restoring a table is the host's to offer: the editor only
/// edits.
///
/// Actions are listed by name: with many of them, finding the one you want
/// beats keeping related ones together. Sorted per rebuild, not per frame.
fn build_rows<A: Action>(
    groups: Groups<A>,
    surfaces: &'static [&'static str],
    store: &Store,
    open: Option<A>,
) -> Vec<Row<A>> {
    let mut rows = Vec::new();
    for (group, members) in groups {
        rows.push(Row::Group(group));
        let mut actions: Vec<&A> = members.iter().collect();
        actions.sort_by_key(|action| action.display());
        for action in actions {
            rows.extend(action_rows(store, surfaces, action, open == Some(*action)));
        }
    }
    rows
}

/// One action's line, and — while it is open — a row per gesture plus its Add.
fn action_rows<A: Action>(
    store: &Store,
    surfaces: &'static [&'static str],
    action: &A,
    open: bool,
) -> Vec<Row<A>> {
    let gestures = bound_gestures(store, surfaces, action);
    let mut rows = vec![Row::Command {
        action: *action,
        gestures: gestures.clone(),
        open,
    }];
    if !open {
        return rows;
    }
    rows.extend(gestures.into_iter().map(|bound| match bound {
        Bound {
            text,
            source: Source::Surface(surface),
            suppressed: true,
        } => Row::Suppressed { text, surface },
        Bound { text, source, .. } => Row::Gesture { text, source },
    }));
    rows.push(Row::Add(*action));
    rows
}

/// Everything bound to an action, base tables first and surface overrides after.
fn bound_gestures<A: Action>(
    store: &Store,
    surfaces: &'static [&'static str],
    action: &A,
) -> Vec<Bound> {
    let name = action.name();
    let mut bound = Vec::new();
    for (table, source) in [
        (&store.gamepad, Source::Gamepad),
        (&store.keyboard, Source::Keyboard),
    ] {
        for (text, _) in table.iter().filter(|(_, bound)| *bound == name) {
            bound.push(Bound {
                text: text.clone(),
                source,
                suppressed: false,
            });
        }
    }
    for surface in surfaces {
        let Some(table) = store.surface.get(*surface) else {
            continue;
        };
        for (text, on) in table {
            // Switched off here, but still this action's gesture everywhere
            // else.
            let suppressed = on == UNBOUND && store.gamepad.get(text).is_some_and(|b| b == name);
            if on == name || suppressed {
                bound.push(Bound {
                    text: text.clone(),
                    source: Source::Surface(surface),
                    suppressed,
                });
            }
        }
    }
    bound
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::TestAction;

    const GROUPS: Groups<TestAction> = &[
        ("General", &[TestAction::Confirm, TestAction::NavDown]),
        (
            "Reading",
            &[
                TestAction::PageNext,
                TestAction::ThemeNext,
                TestAction::Click,
            ],
        ),
    ];
    const SURFACES: &[&str] = &["reader"];

    fn controls() -> Controls<TestAction> {
        // No trailing rows: these tests are about the editor's own.
        Controls::new(GROUPS, SURFACES, 0)
    }

    fn store() -> Store {
        let mut store = Store::default();
        store.gamepad.insert("a".into(), "confirm".into());
        store.gamepad.insert("x".into(), "page_next".into());
        store.keyboard.insert("ctrl+t".into(), "theme_next".into());
        store
            .surface
            .entry("reader".into())
            .or_default()
            .insert("a".into(), "none".into());
        store
    }

    /// What the line for `name` carries, suppressed entries aside.
    fn gestures_under(controls: &Controls<TestAction>, name: &str) -> Vec<(String, Source)> {
        bound_under(controls, name)
            .iter()
            .filter(|b| !b.suppressed)
            .map(|b| (b.text.clone(), b.source))
            .collect()
    }

    fn bound_under<'a>(controls: &'a Controls<TestAction>, name: &str) -> &'a [Bound] {
        controls
            .rows()
            .iter()
            .find_map(|r| match r {
                Row::Command {
                    action, gestures, ..
                } if action.name() == name => Some(gestures.as_slice()),
                _ => None,
            })
            .expect("action is listed")
    }

    #[test]
    fn every_action_gets_one_line_and_nothing_more_until_it_opens() {
        let mut c = controls();
        c.show(&store());
        let commands = c
            .rows()
            .iter()
            .filter(|r| matches!(r, Row::Command { .. }))
            .count();
        assert_eq!(commands, TestAction::all().len());
        assert_eq!(c.rows().len(), commands + GROUPS.len());

        c.toggle_command(TestAction::Confirm, &store());
        // Its own gestures and its Add row, and no other action's.
        assert_eq!(
            c.rows().len(),
            commands + GROUPS.len() + bound_under(&c, "confirm").len() + 1
        );
        assert_eq!(
            c.rows().iter().filter(|r| matches!(r, Row::Add(_))).count(),
            1
        );
    }

    #[test]
    fn opening_a_second_command_closes_the_first() {
        let mut c = controls();
        let s = store();
        c.show(&s);
        c.toggle_command(TestAction::Confirm, &s);
        c.toggle_command(TestAction::PageNext, &s);
        let open: Vec<&str> = c
            .rows()
            .iter()
            .filter_map(|r| match r {
                Row::Command { action, open, .. } if *open => Some(action.name()),
                _ => None,
            })
            .collect();
        assert_eq!(open, ["page_next"]);
    }

    #[test]
    fn toggling_the_open_command_closes_it() {
        let mut c = controls();
        let s = store();
        c.show(&s);
        let collapsed = c.rows().len();
        c.toggle_command(TestAction::Confirm, &s);
        assert!(c.rows().len() > collapsed);
        c.toggle_command(TestAction::Confirm, &s);
        assert_eq!(c.rows().len(), collapsed);
    }

    #[test]
    fn reopening_the_screen_collapses_it_again() {
        let mut c = controls();
        let s = store();
        c.show(&s);
        let collapsed = c.rows().len();
        c.toggle_command(TestAction::Confirm, &s);
        c.close();
        c.show(&s);
        assert_eq!(c.rows().len(), collapsed);
    }

    #[test]
    fn actions_are_grouped_and_named_within_each_group() {
        let mut c = controls();
        c.show(&store());
        let mut groups: Vec<&str> = Vec::new();
        let mut per_group: Vec<Vec<&str>> = Vec::new();
        for row in c.rows() {
            match row {
                Row::Group(name) => {
                    groups.push(name);
                    per_group.push(Vec::new());
                }
                Row::Command { action, .. } => per_group
                    .last_mut()
                    .expect("a group opens before any action")
                    .push(action.display()),
                _ => {}
            }
        }
        let expected: Vec<&str> = GROUPS.iter().map(|(name, _)| *name).collect();
        assert_eq!(groups, expected, "sections come in their declared order");
        for (group, actions) in groups.iter().zip(&per_group) {
            let mut sorted = actions.clone();
            sorted.sort_unstable();
            assert_eq!(actions, &sorted, "`{group}` is not in name order");
        }
    }

    #[test]
    fn a_gesture_is_listed_under_the_action_it_is_bound_to() {
        let mut c = controls();
        c.show(&store());
        assert_eq!(
            gestures_under(&c, "confirm"),
            [("a".to_string(), Source::Gamepad)]
        );
        assert_eq!(
            gestures_under(&c, "page_next"),
            [("x".to_string(), Source::Gamepad)]
        );
        assert_eq!(
            gestures_under(&c, "theme_next"),
            [("ctrl+t".to_string(), Source::Keyboard)]
        );
    }

    #[test]
    fn an_override_is_shown_so_the_screen_does_not_hide_it() {
        // `[surface.reader] a = "none"` switches A off there; unmarked, the
        // screen would still show A under confirm and read as a lie.
        let mut c = controls();
        let s = store();
        c.show(&s);
        assert!(
            bound_under(&c, "confirm")
                .iter()
                .any(|b| b.text == "a" && b.suppressed),
            "the collapsed line should carry the reader override"
        );
        c.toggle_command(TestAction::Confirm, &s);
        assert!(
            c.rows()
                .iter()
                .any(|r| matches!(r, Row::Suppressed { text, surface }
                if text == "a" && *surface == "reader")),
            "opening it should give the override a row"
        );
        // The base binding is still listed too: A confirms, just not there.
        assert_eq!(
            gestures_under(&c, "confirm"),
            [("a".to_string(), Source::Gamepad)]
        );
    }

    #[test]
    fn the_cursor_skips_groups_and_wraps() {
        let mut c = controls();
        c.show(&store());
        assert!(c.selected().is_some_and(|r| r.selectable()));
        // Back off the top lands on the last row, and forward returns.
        c.move_cursor(-1);
        assert_eq!(c.cursor(), *c.selectable.last().expect("rows exist"));
        assert!(c.selected().is_some_and(|r| r.selectable()));
        c.move_cursor(1);
        assert_eq!(c.cursor(), c.selectable[0]);
        // A page jump wraps by the same arithmetic, landing on a row either way.
        c.move_cursor(-5);
        assert_eq!(c.cursor(), c.selectable[c.selectable.len() - 5]);
        for _ in 0..c.rows().len() * 2 {
            c.move_cursor(1);
            assert!(c.selected().is_some_and(|r| r.selectable()));
        }
    }

    #[test]
    fn a_tap_on_a_group_is_ignored() {
        let mut c = controls();
        c.show(&store());
        let group = c
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Group(_)))
            .expect("a group exists");
        let before = c.cursor();
        c.set_cursor(group);
        assert_eq!(c.cursor(), before);
    }

    #[test]
    fn removing_a_gesture_leaves_the_cursor_on_a_real_row() {
        let mut c = controls();
        let mut s = store();
        c.show(&s);
        c.toggle_command(TestAction::Confirm, &s);
        // Sit on the gamepad gesture under confirm, then unbind it.
        let row = c
            .rows()
            .iter()
            .position(|r| {
                matches!(r, Row::Gesture { text, source }
                if text == "a" && *source == Source::Gamepad)
            })
            .expect("the row exists");
        c.set_cursor(row);
        s.gamepad.remove("a");
        c.rebuild(&s);
        assert!(c.selected().is_some_and(|r| r.selectable()));
        assert!(gestures_under(&c, "confirm").is_empty());
    }

    /// One gesture per requirement, so any removal is refused.
    const REQUIRED: &[Requirement<TestAction>] = &[
        ("Confirm", &[TestAction::Confirm]),
        ("Moving the cursor", &[TestAction::NavDown]),
    ];

    fn minimal() -> Table {
        [
            ("a", TestAction::Confirm),
            ("down", TestAction::NavDown),
            ("x", TestAction::PageNext),
        ]
        .into_iter()
        .map(|(g, a)| (g.to_string(), a.name().to_string()))
        .collect()
    }

    #[test]
    fn a_requirements_last_gesture_cannot_be_unbound_or_rebound_away() {
        assert_eq!(
            requirement_lost(&minimal(), "a", None, REQUIRED),
            Some("Confirm")
        );
        assert_eq!(
            requirement_lost(&minimal(), "a", Some(TestAction::PageNext), REQUIRED),
            Some("Confirm")
        );
        assert_eq!(
            requirement_lost(&minimal(), "down", None, REQUIRED),
            Some("Moving the cursor")
        );
    }

    #[test]
    fn an_action_nothing_depends_on_is_free_to_go() {
        assert_eq!(requirement_lost(&minimal(), "x", None, REQUIRED), None);
    }

    #[test]
    fn a_spare_gesture_makes_the_edit_safe() {
        let mut t = minimal();
        t.insert("hold:y".into(), TestAction::Confirm.name().into());
        assert_eq!(requirement_lost(&t, "a", None, REQUIRED), None);
    }

    #[test]
    fn rebinding_a_gesture_to_the_same_action_is_not_a_loss() {
        assert_eq!(
            requirement_lost(&minimal(), "a", Some(TestAction::Confirm), REQUIRED),
            None
        );
    }

    #[test]
    fn a_brand_new_gesture_can_only_add() {
        // Not in the table yet, so nothing can be losing it.
        assert_eq!(
            requirement_lost(&minimal(), "l3", Some(TestAction::ThemeNext), REQUIRED),
            None
        );
    }

    #[test]
    fn a_table_is_checked_as_it_stands() {
        assert!(meets_every_requirement(&minimal(), REQUIRED));
        let mut t = minimal();
        t.remove("down");
        assert!(!meets_every_requirement(&t, REQUIRED));
    }
}

#[cfg(test)]
mod validate_tests {
    use super::*;
    use crate::testkit::TestAction;
    use crate::{Bindings, PadGesture, Store};

    const REQUIRED: &[Requirement<TestAction>] = &[("Confirm", &[TestAction::Confirm])];

    fn table(pairs: &[(&str, TestAction)]) -> Table {
        pairs
            .iter()
            .map(|(g, a)| (g.to_string(), a.name().to_string()))
            .collect()
    }

    #[test]
    fn a_gesture_deferring_a_press_edge_tap_is_refused_with_the_pad() {
        // R1 pages, so a hold or a chord it leads would move the page turn.
        let t = table(&[("r1", TestAction::PageNext), ("a", TestAction::Confirm)]);
        assert_eq!(
            validate(&t, "hold:r1", Some(TestAction::ThemeNext), REQUIRED),
            Err(Refusal::PressEdge(Pad::R1))
        );
        assert_eq!(
            validate(&t, "r1+start", Some(TestAction::ThemeNext), REQUIRED),
            Err(Refusal::PressEdge(Pad::R1))
        );
        assert_eq!(
            validate(&t, "start+r1", Some(TestAction::ThemeNext), REQUIRED),
            Ok(())
        );
        assert_eq!(
            validate(&t, "hold:y", Some(TestAction::ThemeNext), REQUIRED),
            Ok(())
        );
    }

    #[test]
    fn losing_a_requirements_last_gesture_is_refused_with_its_label() {
        let t = table(&[("a", TestAction::Confirm), ("x", TestAction::PageNext)]);
        assert_eq!(
            validate(&t, "a", None, REQUIRED),
            Err(Refusal::Requirement("Confirm"))
        );
        assert_eq!(
            validate(&t, "a", Some(TestAction::PageNext), REQUIRED),
            Err(Refusal::Requirement("Confirm"))
        );
        assert_eq!(validate(&t, "x", None, REQUIRED), Ok(()));
        let mut spare = t.clone();
        spare.insert("b".into(), TestAction::Confirm.name().into());
        assert_eq!(validate(&spare, "a", None, REQUIRED), Ok(()));
    }

    #[test]
    fn unbinding_skips_the_press_edge_rule() {
        let t = table(&[
            ("r1", TestAction::PageNext),
            ("hold:r1", TestAction::ThemeNext),
            ("a", TestAction::Confirm),
        ]);
        assert_eq!(validate(&t, "hold:r1", None, REQUIRED), Ok(()));
    }

    /// The two press-edge implementations read different shapes of the same
    /// table, so they are held to the same answer rather than trusted to agree.
    #[test]
    fn the_table_and_the_built_tables_refuse_the_same_gestures() {
        let store = Store {
            gamepad: table(&[
                ("r1", TestAction::PageNext),
                ("down", TestAction::NavDown),
                ("a", TestAction::Confirm),
                ("y", TestAction::ThemeNext),
            ]),
            ..Store::default()
        };
        let built: Bindings<TestAction> = Bindings::new(&store, &[], |_| None);
        for text in [
            "hold:r1",
            "hold:down",
            "hold:a",
            "hold:y",
            "r1+start",
            "start+r1",
            "down+a",
            "a",
            "start",
        ] {
            let gesture = PadGesture::parse(text).expect("valid gesture");
            assert_eq!(
                press_edge_conflict::<TestAction>(&store.gamepad, text),
                built.press_edge_conflict(gesture),
                "disagreement on `{text}`"
            );
        }
    }
}

#[cfg(test)]
mod trailing_tests {
    use super::*;
    use crate::testkit::TestAction;

    const GROUPS: Groups<TestAction> = &[("General", &[TestAction::Confirm, TestAction::NavDown])];
    /// As a host's "restore the gamepad / keyboard defaults" pair.
    const TRAILING: usize = 2;

    fn controls() -> Controls<TestAction> {
        let mut c = Controls::new(GROUPS, &[], TRAILING);
        c.show(&Store::default());
        c
    }

    #[test]
    fn the_cursor_reaches_the_hosts_rows_past_the_editors_own() {
        let mut c = controls();
        let last_own = c.rows().len() - 1;
        // The wrap is what makes them cheap to reach: one step back off the top.
        c.move_cursor(-1);
        assert_eq!(c.cursor(), last_own + TRAILING);
        assert_eq!(c.trailing_cursor(), Some(TRAILING - 1));
        // A trailing row is not the editor's, so it has none to hand back.
        assert!(c.selected().is_none());
    }

    #[test]
    fn stepping_back_off_a_trailing_row_lands_on_the_editors_last() {
        let mut c = controls();
        c.move_cursor(-1);
        for _ in 0..TRAILING {
            c.move_cursor(-1);
        }
        assert_eq!(c.trailing_cursor(), None);
        assert!(c.selected().is_some_and(|r| r.selectable()));
    }

    #[test]
    fn a_click_can_land_on_a_trailing_row() {
        let mut c = controls();
        let first_trailing = c.rows().len();
        c.set_cursor(first_trailing);
        assert_eq!(c.trailing_cursor(), Some(0));
        c.set_cursor(first_trailing + TRAILING);
        assert_eq!(c.trailing_cursor(), Some(0));
    }

    /// The rows shift under a trailing cursor when a command opens; it must not
    /// be left pointing past the end.
    #[test]
    fn a_trailing_cursor_survives_a_rebuild() {
        let mut c = controls();
        let store = Store::default();
        c.set_cursor(c.rows().len() + TRAILING - 1);
        c.toggle_command(TestAction::Confirm, &store);
        assert_eq!(c.trailing_cursor(), Some(TRAILING - 1));
        c.toggle_command(TestAction::Confirm, &store);
        assert_eq!(c.trailing_cursor(), Some(TRAILING - 1));
    }

    #[test]
    fn no_trailing_rows_leaves_the_cursor_inside_the_editor() {
        let mut c = Controls::new(GROUPS, &[], 0);
        c.show(&Store::default());
        for _ in 0..c.rows().len() * 2 {
            c.move_cursor(1);
        }
        assert_eq!(c.trailing_cursor(), None);
        assert!(c.selected().is_some());
    }
}

#[cfg(test)]
mod mods_tests {
    use crate::Mods;

    #[test]
    fn only_ctrl_and_alt_stop_a_gesture_being_plain() {
        assert!(Mods::NONE.is_plain());
        // Shift is part of ordinary typing, so it stays plain.
        assert!(Mods::SHIFT.is_plain());
        assert!(!Mods::CTRL.is_plain());
        assert!(!Mods::ALT.is_plain());
        assert!(!Mods::CTRL.union(Mods::SHIFT).is_plain());
    }
}
