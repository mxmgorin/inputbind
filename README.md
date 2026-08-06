# inputbind

Rebindable input for games and handheld apps: gestures on a pad or a keyboard
resolve to the host app's own action type.

The app supplies three things — an `Action` vocabulary, the default `Store`,
and a key-name resolver. Everything else lives here:

- **Pad gestures**: tap, `hold:y`, ordered chords (`select+x`), with
  press-edge deferral rules, auto-repeat, and analog stick/trigger folding
  with hysteresis.
- **Keyboard**: modifier combos (`ctrl+shift+t`) resolved to backend key
  codes at load.
- **Capture**: listen for the next gesture to bind, with idle give-up so an
  accidental capture cannot trap input on a device with no Esc.
- **A TOML bindings file** (`[gamepad]`, `[keyboard]`, per-surface
  `[surface.<name>]` overrides) that survives hand-editing: a bad line is
  logged and skipped, never fatal.
- Runtime tables are built once and hold no strings; the input path never
  allocates. Time comes in through `now`, so tests drive it directly.
- SDL2 backend behind the `sdl2` feature (on by default); the core names no
  backend.
