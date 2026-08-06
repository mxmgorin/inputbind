//! The bindings file: gesture text → action name, one table per input device
//! plus the optional per-surface overrides.
//!
//! ```toml
//! [gamepad]
//! a = "confirm"
//! "hold:y" = "theme_next"
//!
//! [keyboard]
//! "ctrl+r" = "settings"
//!
//! [surface.reader]      # overrides while that surface is in front
//! a = "none"            # explicitly nothing, rather than falling through
//! ```
//!
//! Text is the whole point of this type: it is what the file holds and what the
//! editor edits. `BTreeMap` so a written file sorts stably. Nothing here is
//! read on the input path — [`super::Bindings`] parses it once into tables that
//! hold no strings at all.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Action name meaning "explicitly nothing" — it suppresses a base binding in a
/// surface override, where an absent key would fall through instead.
pub const UNBOUND: &str = "none";

pub type Table = BTreeMap<String, String>;

/// A section with nothing in it is not written: a bare `[surface]` would name a
/// table nobody edits directly — overrides go under `[surface.<name>]` — and read
/// as a stub to fill in the wrong shape.
#[derive(Default, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    #[serde(skip_serializing_if = "Table::is_empty")]
    pub gamepad: Table,
    #[serde(skip_serializing_if = "Table::is_empty")]
    pub keyboard: Table,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub surface: BTreeMap<String, Table>,
}

impl Store {
    /// Read the file, or write `defaults` as a template and use those. A
    /// malformed file logs and falls back without overwriting it — a typo
    /// should not cost the user their table.
    pub fn load(path: impl AsRef<Path>, defaults: impl FnOnce() -> Store) -> Store {
        let path = path.as_ref();
        let Ok(text) = std::fs::read_to_string(path) else {
            let store = defaults();
            store.write(path, "default bindings");
            return store;
        };
        match toml::from_str::<Store>(&text) {
            Ok(store) => {
                store.report_unknown_tables(&text);
                log::info!("loaded bindings from `{}`", path.display());
                store
            }
            Err(e) => {
                log::error!("invalid bindings `{}`: {e}; using defaults", path.display());
                defaults()
            }
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) {
        self.write(path.as_ref(), "bindings");
    }

    fn write(&self, path: &Path, what: &str) {
        match toml::to_string_pretty(self) {
            Ok(text) => match std::fs::write(path, text) {
                Ok(()) => log::info!("wrote {what} to `{}`", path.display()),
                Err(e) => log::warn!("could not write {what} `{}`: {e}", path.display()),
            },
            Err(e) => log::warn!("could not serialize {what}: {e}"),
        }
    }

    /// A mistyped table name deserializes to nothing and would be dropped on
    /// the next save, so name it while the file still has it.
    fn report_unknown_tables(&self, text: &str) {
        let Ok(raw) = text.parse::<toml::Table>() else {
            return;
        };
        for key in raw.keys() {
            if !matches!(key.as_str(), "gamepad" | "keyboard" | "surface") {
                log::warn!("bindings: unknown table `{key}`; ignored and dropped on save");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_round_trips_through_toml() {
        let mut store = Store::default();
        store.gamepad.insert("a".into(), "confirm".into());
        store.gamepad.insert("hold:y".into(), "theme_next".into());
        store.keyboard.insert("ctrl+r".into(), "settings".into());
        store
            .surface
            .entry("reader".into())
            .or_default()
            .insert("a".into(), UNBOUND.into());

        let text = toml::to_string_pretty(&store).expect("serializable");
        assert_eq!(toml::from_str::<Store>(&text).expect("parses"), store);
    }

    #[test]
    fn an_empty_section_is_not_written() {
        let mut store = Store::default();
        store.gamepad.insert("a".into(), "accept".into());
        let text = toml::to_string_pretty(&store).expect("serializable");
        assert!(text.contains("[gamepad]"));
        assert!(
            !text.contains("[surface]"),
            "wrote an empty section:\n{text}"
        );
        assert!(
            !text.contains("[keyboard]"),
            "wrote an empty section:\n{text}"
        );
        // Still round-trips: an absent section reads back as empty.
        assert_eq!(toml::from_str::<Store>(&text).expect("parses"), store);
    }

    #[test]
    fn missing_tables_default_to_empty() {
        let store: Store = toml::from_str("[gamepad]\na = \"confirm\"\n").expect("parses");
        assert_eq!(store.gamepad.len(), 1);
        assert!(store.keyboard.is_empty());
        assert!(store.surface.is_empty());
    }
}
