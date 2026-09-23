//! A control surface's protocol: what its encoders and keys say, what its
//! lamps and display are told. Bytes in, bytes out: nothing here opens a
//! MIDI port or draws a widget.
//!
//! The vocabulary at the root is what every surface has in common. What a
//! protocol says on the wire, and what its display can show, lives in the
//! protocol's own module.

pub mod hypercontrol;
pub mod pager;

pub use pager::Pager;

/// A control the surface has of its own, by role rather than by wire number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Play,
    Stop,
    Record,
    Loop,
    Rewind,
    FastForward,
    BankLeft,
    BankRight,
    /// One of the eight select keys, 0..7.
    Select(u8),
    /// One of the four keys under the display, 0..3.
    Device(u8),
    Peek,
    Display,
    Mute,
    Solo,
    Mixer,
    SelectLeft,
    SelectRight,
    Mode,
}

/// What a surface sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Encoder `index`, 0..7, turned `delta` detents, negative anticlockwise.
    /// `delta` is never zero.
    Encoder { index: u8, delta: i8 },
    Key { key: Key, down: bool },
}

/// Positions an encoder cuts a continuous control into: one per detent from
/// one end to the other.
pub const DETENTS: u32 = 127;

/// `value`, normalised 0..1, moved `delta` detents. A continuous control
/// (`steps == 0`) moves by `1 / DETENTS` per detent; a stepped one moves
/// one step per detent and lands on a step. The result stays in 0..1.
pub fn nudge(value: f64, steps: u32, delta: i32) -> f64 {
    if steps == 0 {
        (value + delta as f64 / DETENTS as f64).clamp(0.0, 1.0)
    } else {
        let at = (value * steps as f64).round() as i64 + delta as i64;
        at.clamp(0, steps as i64) as f64 / steps as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A continuous control moves a detent at a time and stops at the ends.
    #[test]
    fn a_continuous_control_moves_by_detents() {
        assert!((nudge(0.0, 0, 1) - 1.0 / 127.0).abs() < 1e-12);
        assert!((nudge(0.5, 0, -3) - (0.5 - 3.0 / 127.0)).abs() < 1e-12);
        assert_eq!(nudge(0.0, 0, -1), 0.0);
        assert_eq!(nudge(1.0, 0, 1), 1.0);
        assert_eq!(nudge(0.999, 0, 5), 1.0);
    }

    /// A stepped control moves a step at a time, lands on a step and stops
    /// at the ends.
    #[test]
    fn a_stepped_control_moves_by_steps() {
        assert_eq!(nudge(0.0, 1, 1), 1.0, "a switch turns on");
        assert_eq!(nudge(1.0, 1, -1), 0.0, "and off");
        assert_eq!(nudge(1.0, 1, 1), 1.0);
        assert!((nudge(0.0, 3, 1) - 1.0 / 3.0).abs() < 1e-12);
        assert!((nudge(0.4, 3, 0) - 1.0 / 3.0).abs() < 1e-12, "snaps to the nearest step");
        assert_eq!(nudge(1.0, 3, 5), 1.0);
        assert_eq!(nudge(0.0, 3, -5), 0.0);
    }
}
