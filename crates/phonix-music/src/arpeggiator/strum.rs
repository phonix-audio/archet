//! Chord strum enhancement — control how chord notes are spread in time.

/// Strum direction for chord mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrumDirection {
    /// Low to high
    Up,
    /// High to low
    Down,
    /// Alternates up/down each step
    Alternating,
}

impl StrumDirection {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Up,
            1 => Self::Down,
            _ => Self::Alternating,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Up => 0,
            Self::Down => 1,
            Self::Alternating => 2,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Alternating => "Alternating",
        }
    }

    pub const COUNT: usize = 3;
}

/// Strum configuration for chord mode
#[derive(Debug, Clone)]
pub struct StrumConfig {
    /// 0.0 = instant chord, 1.0 = notes spread across full step
    pub speed: f32,
    pub direction: StrumDirection,
    /// Internal: tracks alternating direction state
    pub(crate) alternating_up: bool,
}

impl StrumConfig {
    pub fn new() -> Self {
        Self {
            speed: 0.0,
            direction: StrumDirection::Up,
            alternating_up: true,
        }
    }

    /// Get the sample delay for the i-th note in a chord of `count` notes,
    /// given the step duration in samples.
    pub fn note_delay(&self, note_index: usize, chord_size: usize, step_samples: usize) -> usize {
        if self.speed <= 0.0 || chord_size <= 1 {
            return 0;
        }

        let max_spread = (step_samples as f32 * self.speed * 0.5) as usize; // max half the step
        let spacing = max_spread / chord_size.max(1);

        let effective_up = match self.direction {
            StrumDirection::Up => true,
            StrumDirection::Down => false,
            StrumDirection::Alternating => self.alternating_up,
        };

        let idx = if effective_up {
            note_index
        } else {
            chord_size - 1 - note_index
        };

        idx * spacing
    }

    /// Advance alternating direction (call once per chord step)
    pub fn advance_alternating(&mut self) {
        self.alternating_up = !self.alternating_up;
    }
}
