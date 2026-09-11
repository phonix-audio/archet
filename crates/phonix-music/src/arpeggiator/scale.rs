//! Scale quantization — snap arpeggiator output to musical scales.

/// Available scale modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
    /// No quantization (chromatic pass-through)
    Chromatic,
    Major,
    NaturalMinor,
    HarmonicMinor,
    MelodicMinor,
    Pentatonic,
    MinorPentatonic,
    Blues,
    WholeTone,
    /// User-defined interval set
    Custom,
}

impl ScaleMode {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Chromatic,
            1 => Self::Major,
            2 => Self::NaturalMinor,
            3 => Self::HarmonicMinor,
            4 => Self::MelodicMinor,
            5 => Self::Pentatonic,
            6 => Self::MinorPentatonic,
            7 => Self::Blues,
            8 => Self::WholeTone,
            _ => Self::Custom,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Chromatic => 0,
            Self::Major => 1,
            Self::NaturalMinor => 2,
            Self::HarmonicMinor => 3,
            Self::MelodicMinor => 4,
            Self::Pentatonic => 5,
            Self::MinorPentatonic => 6,
            Self::Blues => 7,
            Self::WholeTone => 8,
            Self::Custom => 9,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Chromatic => "Chromatic",
            Self::Major => "Major",
            Self::NaturalMinor => "Minor",
            Self::HarmonicMinor => "Harmonic Min",
            Self::MelodicMinor => "Melodic Min",
            Self::Pentatonic => "Pentatonic",
            Self::MinorPentatonic => "Minor Penta",
            Self::Blues => "Blues",
            Self::WholeTone => "Whole Tone",
            Self::Custom => "Custom",
        }
    }

    /// Get the interval pattern for this scale (which of the 12 semitones are in-scale)
    fn intervals(&self) -> [bool; 12] {
        match self {
            Self::Chromatic     => [true, true, true, true, true, true, true, true, true, true, true, true],
            Self::Major         => [true, false, true, false, true, true, false, true, false, true, false, true],
            Self::NaturalMinor  => [true, false, true, true, false, true, false, true, true, false, true, false],
            Self::HarmonicMinor => [true, false, true, true, false, true, false, true, true, false, false, true],
            Self::MelodicMinor  => [true, false, true, true, false, true, false, true, false, true, false, true],
            Self::Pentatonic    => [true, false, true, false, true, false, false, true, false, true, false, false],
            Self::MinorPentatonic => [true, false, false, true, false, true, false, true, false, false, true, false],
            Self::Blues          => [true, false, false, true, false, true, true, true, false, false, true, false],
            Self::WholeTone     => [true, false, true, false, true, false, true, false, true, false, true, false],
            Self::Custom        => [true, true, true, true, true, true, true, true, true, true, true, true],
        }
    }

    pub const COUNT: usize = 10;
}

/// Scale quantizer: snaps MIDI notes to the nearest in-scale note.
#[derive(Debug, Clone)]
pub struct ScaleQuantizer {
    pub mode: ScaleMode,
    /// Root note (0=C, 1=C#, ... 11=B)
    pub root: u8,
    /// Custom interval set (only used when mode == Custom)
    pub custom_intervals: [bool; 12],
}

impl ScaleQuantizer {
    pub fn new() -> Self {
        Self {
            mode: ScaleMode::Chromatic,
            root: 0,
            custom_intervals: [true; 12],
        }
    }

    /// Get the active interval set (built-in or custom)
    fn active_intervals(&self) -> [bool; 12] {
        if self.mode == ScaleMode::Custom {
            self.custom_intervals
        } else {
            self.mode.intervals()
        }
    }

    /// Quantize a MIDI note to the nearest note in the active scale.
    /// Prefers the note itself, then the nearest lower note, then upper.
    pub fn quantize(&self, note: u8) -> u8 {
        if self.mode == ScaleMode::Chromatic {
            return note;
        }
        let intervals = self.active_intervals();
        let degree = ((note as i32 - self.root as i32).rem_euclid(12)) as usize;

        // Already in scale
        if intervals[degree] {
            return note;
        }

        // Search outward: prefer down, then up
        for offset in 1..=6 {
            let down = (degree + 12 - offset) % 12;
            if intervals[down] {
                return (note as i32 - offset as i32).max(0) as u8;
            }
            let up = (degree + offset) % 12;
            if intervals[up] {
                return (note as i32 + offset as i32).min(127) as u8;
            }
        }
        note // fallback (shouldn't happen with any scale that has notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromatic_passthrough() {
        let q = ScaleQuantizer::new();
        for n in 0..=127 {
            assert_eq!(q.quantize(n), n);
        }
    }

    #[test]
    fn c_major_quantize() {
        let mut q = ScaleQuantizer::new();
        q.mode = ScaleMode::Major;
        q.root = 0; // C
        // C D E F G A B = 0 2 4 5 7 9 11
        assert_eq!(q.quantize(60), 60); // C4 → C4
        assert_eq!(q.quantize(61), 60); // C#4 → C4 (down)
        assert_eq!(q.quantize(63), 62); // Eb4 → D4 (down)
        assert_eq!(q.quantize(66), 65); // F#4 → F4 (down)
    }
}
