//! The pad read from a Linux kernel event node, for hosts whose display
//! layer's own input is lossy. The Miyoo's bundled SDL samples its keypad as
//! levels once per pump, so a press and release that both land between two
//! pumps cancel out; the kernel's queue loses nothing and fans out to every
//! reader. The host opens the node, parks a thread in [`read_edges`], and
//! forwards each edge into its own event loop.

use super::pad::{Edge, Pad};
use std::io::Read;
use std::os::raw::c_long;

/// `struct input_event` as the kernel hands it to a matching userland: a
/// native-`long` timeval, then type, code, value.
#[repr(C)]
struct InputEvent {
    tv_sec: c_long,
    tv_usec: c_long,
    kind: u16,
    code: u16,
    value: i32,
}

const EVENT_SIZE: usize = std::mem::size_of::<InputEvent>();

/// `EV_KEY`.
const EV_KEY: u16 = 1;
/// Key edge values; `2` is the kernel's own autorepeat, which is dropped —
/// repeat cadence is the gesture machine's to decide.
const RELEASED: i32 = 0;
const PRESSED: i32 = 1;

/// Linux `KEY_*` codes, as `input-event-codes.h` spells them.
const KEY_ESC: u16 = 1;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_E: u16 = 18;
const KEY_T: u16 = 20;
const KEY_ENTER: u16 = 28;
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_LEFTALT: u16 = 56;
const KEY_SPACE: u16 = 57;
const KEY_RIGHTCTRL: u16 = 97;
const KEY_UP: u16 = 103;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_DOWN: u16 = 108;

/// Which device's keypad the kernel codes describe — the same role
/// `sdl::Keymap` plays for the keys SDL makes of them.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Keymap {
    MiyooMini,
}

impl Keymap {
    /// The pad a kernel key code stands for, `None` for a key that is not the
    /// pad's (MENU stays a system key on the Miyoo).
    pub fn pad(self, code: u16) -> Option<Pad> {
        match self {
            Keymap::MiyooMini => miyoo(code),
        }
    }
}

fn miyoo(code: u16) -> Option<Pad> {
    Some(match code {
        // MENU is the system kill helper's, not a pad.
        KEY_ESC => return None,
        KEY_UP => Pad::Up,
        KEY_DOWN => Pad::Down,
        KEY_LEFT => Pad::Left,
        KEY_RIGHT => Pad::Right,
        KEY_SPACE => Pad::A,
        KEY_LEFTCTRL => Pad::B,
        KEY_LEFTSHIFT => Pad::X,
        KEY_LEFTALT => Pad::Y,
        KEY_E => Pad::L1,
        KEY_T => Pad::R1,
        KEY_TAB => Pad::L2,
        KEY_BACKSPACE => Pad::R2,
        KEY_ENTER => Pad::Start,
        KEY_RIGHTCTRL => Pad::Select,
        _ => return None,
    })
}

/// Read `source` until it ends, handing each keypad edge to `on_edge`. Blocks,
/// so the host gives it a thread; a kernel node never ends on its own, and
/// `Ok` is the stream closing under us.
pub fn read_edges(
    mut source: impl Read,
    keymap: Keymap,
    mut on_edge: impl FnMut(Pad, Edge),
) -> std::io::Result<()> {
    let mut raw = [0u8; EVENT_SIZE];
    loop {
        // The kernel writes whole events; `read_exact` reassembles one from
        // any reader that fragments.
        if let Err(e) = source.read_exact(&mut raw) {
            return match e.kind() {
                std::io::ErrorKind::UnexpectedEof => Ok(()),
                _ => Err(e),
            };
        }
        // A byte array carries no alignment promise for the struct.
        let ev: InputEvent = unsafe { std::ptr::read_unaligned(raw.as_ptr() as *const InputEvent) };
        if ev.kind != EV_KEY || !matches!(ev.value, RELEASED | PRESSED) {
            continue;
        }
        if let Some(pad) = keymap.pad(ev.code) {
            on_edge(
                pad,
                if ev.value == PRESSED {
                    Edge::Press
                } else {
                    Edge::Release
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bytes the kernel would write for one event.
    fn ev(kind: u16, code: u16, value: i32) -> Vec<u8> {
        let mut out = vec![0u8; std::mem::size_of::<c_long>() * 2];
        out.extend_from_slice(&kind.to_ne_bytes());
        out.extend_from_slice(&code.to_ne_bytes());
        out.extend_from_slice(&value.to_ne_bytes());
        out
    }

    fn edges_of(stream: &[u8]) -> Vec<(Pad, Edge)> {
        let mut out = Vec::new();
        read_edges(stream, Keymap::MiyooMini, |pad, edge| out.push((pad, edge)))
            .expect("a slice ends in EOF, never an error");
        out
    }

    #[test]
    fn a_tap_comes_through_and_the_noise_does_not() {
        const EV_SYN: u16 = 0;
        const AUTOREPEAT: i32 = 2;
        let mut stream = Vec::new();
        stream.extend(ev(EV_KEY, KEY_DOWN, PRESSED));
        stream.extend(ev(EV_SYN, 0, 0));
        stream.extend(ev(EV_KEY, KEY_DOWN, AUTOREPEAT));
        stream.extend(ev(EV_KEY, KEY_ESC, PRESSED)); // MENU: the system's
        stream.extend(ev(EV_KEY, KEY_DOWN, RELEASED));
        assert_eq!(
            edges_of(&stream),
            [(Pad::Down, Edge::Press), (Pad::Down, Edge::Release)]
        );
    }

    /// Hands out one byte per read, as a pipe under pressure might.
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.0.len().min(1).min(buf.len());
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn a_fragmented_stream_reassembles() {
        let mut stream = Vec::new();
        stream.extend(ev(EV_KEY, KEY_SPACE, PRESSED));
        stream.extend(ev(EV_KEY, KEY_SPACE, RELEASED));
        let mut out = Vec::new();
        read_edges(Trickle(&stream), Keymap::MiyooMini, |pad, edge| {
            out.push((pad, edge))
        })
        .expect("EOF is a clean end");
        assert_eq!(out, [(Pad::A, Edge::Press), (Pad::A, Edge::Release)]);
    }

    #[cfg(feature = "sdl2")]
    #[test]
    fn the_kernel_and_sdl_tables_agree() {
        use crate::sdl::Keymap as SdlKeymap;
        use sdl2::keyboard::Keycode;
        // A host reading the kernel around SDL must lose no button. The dock
        // keys (PageUp/PageDown) are a real keyboard's, not the keypad's.
        for (key, code) in [
            (Keycode::UP, KEY_UP),
            (Keycode::DOWN, KEY_DOWN),
            (Keycode::LEFT, KEY_LEFT),
            (Keycode::RIGHT, KEY_RIGHT),
            (Keycode::SPACE, KEY_SPACE),
            (Keycode::LCTRL, KEY_LEFTCTRL),
            (Keycode::LSHIFT, KEY_LEFTSHIFT),
            (Keycode::LALT, KEY_LEFTALT),
            (Keycode::E, KEY_E),
            (Keycode::T, KEY_T),
            (Keycode::TAB, KEY_TAB),
            (Keycode::BACKSPACE, KEY_BACKSPACE),
            (Keycode::RETURN, KEY_ENTER),
            (Keycode::RCTRL, KEY_RIGHTCTRL),
        ] {
            assert_eq!(
                Keymap::MiyooMini.pad(code),
                SdlKeymap::MiyooMini.pad(key),
                "code {code}"
            );
        }
    }
}
