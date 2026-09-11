//! Cycle transposition — transpose the pattern by N semitones after each full cycle.

/// Per-cycle transposition: after the note pattern completes one cycle,
/// shift all notes by `semitones`. Resets after `max_cycles` or when
/// notes would exceed MIDI range.
#[derive(Debug, Clone)]
pub struct CycleTranspose {
    /// Semitones to add per cycle (e.g., +7 = fifths, +12 = octaves)
    pub semitones: i8,
    /// Maximum cycles before resetting (0 = unlimited, stop at MIDI range)
    pub max_cycles: u8,
    /// Current cycle count
    pub(crate) current_cycle: u8,
}

impl CycleTranspose {
    pub fn new() -> Self {
        Self {
            semitones: 0,
            max_cycles: 0,
            current_cycle: 0,
        }
    }

    /// Get the current transposition offset in semitones
    pub fn offset(&self) -> i32 {
        if self.semitones == 0 {
            return 0;
        }
        self.semitones as i32 * self.current_cycle as i32
    }

    /// Apply transposition to a note, clamping to valid MIDI range
    pub fn apply(&self, note: u8) -> u8 {
        let transposed = note as i32 + self.offset();
        transposed.clamp(0, 127) as u8
    }

    /// Advance to the next cycle. Returns true if the cycle wrapped (reset).
    pub fn advance_cycle(&mut self) -> bool {
        if self.semitones == 0 {
            return false;
        }
        self.current_cycle += 1;
        if self.max_cycles > 0 && self.current_cycle >= self.max_cycles {
            self.current_cycle = 0;
            return true;
        }
        // Check if next cycle would exceed MIDI range for a typical note (C4=60)
        let test_note = 60i32 + self.offset();
        if test_note < 0 || test_note > 127 {
            self.current_cycle = 0;
            return true;
        }
        false
    }

    /// Reset cycle counter
    pub fn reset(&mut self) {
        self.current_cycle = 0;
    }
}
