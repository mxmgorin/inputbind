//! The bindings editor model: every action with the gestures bound to it, and
//! a row per edit. Pure state — the host draws the rows, runs capture, and
//! applies the edits to the [`Store`]; rebuilding after each edit keeps the
//! screen and the input path from ever disagreeing.
//!
//! Rows are built from the store when the screen opens and after each edit, not
//! per frame: the list is long and a renderer runs on every pass.

use crate::{Action, Store, Table, UNBOUND};

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

#[derive(Clone, Debug)]
pub enum Row<A> {
    /// A section's name. Not selectable.
    Group(&'static str),
    /// An action's name. Not selectable — it labels the rows under it.
    Header(&'static str),
    /// A gesture bound to the action above; activating it unbinds.
    Gesture { text: String, source: Source },
    /// A gesture the base table binds to the action above, switched off by a
    /// `none` override on one surface. Listed here or the screen would show the
    /// binding and hide the fact that it does nothing there.
    Suppressed { text: String, surface: &'static str },
    /// Activating starts listening for a gesture to bind to this action.
    Add(A),
}

impl<A> Row<A> {
    /// Headers label the rows under them; only the rest can be focused, tapped
    /// or activated.
    pub fn selectable(&self) -> bool {
        !matches!(self, Row::Group(_) | Row::Header(_))
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
    groups: Groups<A>,
    surfaces: &'static [&'static str],
}

impl<A: Action> Controls<A> {
    pub fn new(groups: Groups<A>, surfaces: &'static [&'static str]) -> Self {
        Self {
            open: false,
            rows: Vec::new(),
            selectable: Vec::new(),
            cursor: 0,
            capturing: None,
            groups,
            surfaces,
        }
    }

    pub fn show(&mut self, store: &Store) {
        self.rebuild(store);
        self.cursor = self.selectable.first().copied().unwrap_or(0);
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.capturing = None;
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

    /// Step the cursor over selectable rows only; the ends clamp, since the list
    /// is long enough that wrapping would just lose you.
    pub fn move_cursor(&mut self, delta: i32) {
        if self.selectable.is_empty() {
            return;
        }
        let at = self
            .selectable
            .iter()
            .position(|i| *i == self.cursor)
            .unwrap_or(0) as i32;
        let next = (at + delta).clamp(0, self.selectable.len() as i32 - 1);
        self.cursor = self.selectable[next as usize];
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
        self.rows = build_rows(self.groups, self.surfaces, store);
        self.selectable = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.selectable())
            .map(|(i, _)| i)
            .collect();
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

/// A header, the gestures bound to it and an Add row per action. Restoring a
/// table is the host's to offer: the editor only edits.
///
/// Actions are listed by name: with many of them, finding the one you want
/// beats keeping related ones together. Sorted per rebuild, not per frame.
fn build_rows<A: Action>(
    groups: Groups<A>,
    surfaces: &'static [&'static str],
    store: &Store,
) -> Vec<Row<A>> {
    let mut rows = Vec::new();
    for (group, members) in groups {
        rows.push(Row::Group(group));
        let mut actions: Vec<&A> = members.iter().collect();
        actions.sort_by_key(|action| action.display());
        for action in actions {
            rows.extend(action_rows(store, surfaces, action));
        }
    }
    rows
}

/// One action's header, the gestures bound to it, and its Add row.
fn action_rows<A: Action>(
    store: &Store,
    surfaces: &'static [&'static str],
    action: &A,
) -> Vec<Row<A>> {
    let mut rows = vec![Row::Header(action.display())];
    {
        let name = action.name();
        for (text, _) in store.gamepad.iter().filter(|(_, bound)| *bound == name) {
            rows.push(Row::Gesture {
                text: text.clone(),
                source: Source::Gamepad,
            });
        }
        for (text, _) in store.keyboard.iter().filter(|(_, bound)| *bound == name) {
            rows.push(Row::Gesture {
                text: text.clone(),
                source: Source::Keyboard,
            });
        }
        for surface in surfaces {
            let Some(table) = store.surface.get(*surface) else {
                continue;
            };
            for (text, bound) in table {
                if bound == name {
                    rows.push(Row::Gesture {
                        text: text.clone(),
                        source: Source::Surface(surface),
                    });
                } else if bound == UNBOUND && store.gamepad.get(text).is_some_and(|b| b == name) {
                    // Switched off here, but still this action's gesture
                    // everywhere else.
                    rows.push(Row::Suppressed {
                        text: text.clone(),
                        surface,
                    });
                }
            }
        }
        rows.push(Row::Add(*action));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::TestAction;

    const GROUPS: Groups<TestAction> = &[
        ("General", &[TestAction::Confirm, TestAction::NavDown]),
        ("Reading", &[TestAction::PageNext, TestAction::ThemeNext]),
    ];
    const SURFACES: &[&str] = &["reader"];

    fn controls() -> Controls<TestAction> {
        Controls::new(GROUPS, SURFACES)
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

    fn gestures_under(controls: &Controls<TestAction>, display: &str) -> Vec<(String, Source)> {
        let rows = controls.rows();
        let start = rows
            .iter()
            .position(|r| matches!(r, Row::Header(h) if *h == display))
            .expect("action is listed");
        rows[start + 1..]
            .iter()
            .take_while(|r| !matches!(r, Row::Header(_)))
            .filter_map(|r| match r {
                Row::Gesture { text, source } => Some((text.clone(), *source)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_action_gets_a_header_and_an_add_row() {
        let mut c = controls();
        c.show(&store());
        let headers = c
            .rows()
            .iter()
            .filter(|r| matches!(r, Row::Header(_)))
            .count();
        let adds = c.rows().iter().filter(|r| matches!(r, Row::Add(_))).count();
        assert_eq!(headers, TestAction::all().len());
        assert_eq!(adds, TestAction::all().len());
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
                Row::Header(name) => per_group
                    .last_mut()
                    .expect("a group opens before any action")
                    .push(name),
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
        // `[surface.reader] a = "none"` switches A off there; without the row
        // the screen would still show A under confirm and read as a lie.
        let mut c = controls();
        c.show(&store());
        assert!(
            c.rows()
                .iter()
                .any(|r| matches!(r, Row::Suppressed { text, surface }
                if text == "a" && *surface == "reader")),
            "the reader override should have a row"
        );
        // The base binding is still listed too: A confirms, just not there.
        assert_eq!(
            gestures_under(&c, "confirm"),
            [("a".to_string(), Source::Gamepad)]
        );
    }

    #[test]
    fn the_cursor_skips_headers_and_clamps() {
        let mut c = controls();
        c.show(&store());
        assert!(c.selected().is_some_and(|r| r.selectable()));
        c.move_cursor(-5);
        assert_eq!(c.cursor(), c.selectable[0]);
        for _ in 0..c.rows().len() * 2 {
            c.move_cursor(1);
        }
        assert_eq!(c.cursor(), *c.selectable.last().expect("rows exist"));
        assert!(c.selected().is_some_and(|r| r.selectable()));
    }

    #[test]
    fn a_tap_on_a_header_is_ignored() {
        let mut c = controls();
        c.show(&store());
        let header = c
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Header(_)))
            .expect("a header exists");
        let before = c.cursor();
        c.set_cursor(header);
        assert_eq!(c.cursor(), before);
    }

    #[test]
    fn removing_a_gesture_leaves_the_cursor_on_a_real_row() {
        let mut c = controls();
        let mut s = store();
        c.show(&s);
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
