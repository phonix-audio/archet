//! Step sequencer data — per-step velocity, gate, ties, rests, probability, ratchet.

/// What happens on a given step
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepType {
    /// Play the note normally
    Play,
    /// Silence — skip this step
    Rest,
    /// Extend the previous note (no retrigger)
    Tie,
    /// Play with accent velocity boost
    Accent,
}

/// Per-step data in a rhythm pattern
#[derive(Debug, Clone, Copy)]
pub struct StepData {
    pub step_type: StepType,
    /// Per-step velocity override (0 = use global/curve)
    pub velocity: u8,
    /// Per-step gate length override (0.0 = use global)
    pub gate_length: f32,
    /// Probability of this step firing (0.0–1.0)
    pub probability: f32,
    /// Ratchet subdivisions: 1 = normal, 2/3/4 = rapid repeats within the step
    pub ratchet: u8,
}

impl Default for StepData {
    fn default() -> Self {
        Self {
            step_type: StepType::Play,
            velocity: 0,
            gate_length: 0.0,
            probability: 1.0,
            ratchet: 1,
        }
    }
}

/// Maximum steps in a rhythm pattern
pub const MAX_STEPS: usize = 32;

/// A programmable rhythm pattern (step sequencer grid).
/// Fixed-size array — no heap allocation during playback.
#[derive(Debug, Clone)]
pub struct RhythmPattern {
    pub steps: [StepData; MAX_STEPS],
    /// Number of active steps (1–32)
    pub step_count: u8,
    /// Pattern name (short, fixed-size)
    pub name: [u8; 16],
}

impl RhythmPattern {
    pub fn new(step_count: u8) -> Self {
        Self {
            steps: [StepData::default(); MAX_STEPS],
            step_count: step_count.clamp(1, MAX_STEPS as u8),
            name: *b"Default\0\0\0\0\0\0\0\0\0",
        }
    }

    /// Get the step data for a given position (wraps around step_count)
    pub fn get_step(&self, index: usize) -> &StepData {
        &self.steps[index % self.step_count as usize]
    }

    /// Set step data at a position
    pub fn set_step(&mut self, index: usize, data: StepData) {
        if index < self.step_count as usize {
            self.steps[index] = data;
        }
    }

    /// Create a common "four on the floor" pattern
    pub fn four_on_floor() -> Self {
        let mut p = Self::new(16);
        // Accent on beats, play on off-beats
        for i in 0..16 {
            p.steps[i].step_type = if i % 4 == 0 {
                StepType::Accent
            } else {
                StepType::Play
            };
        }
        p.name = *b"4 on the floor\0\0";
        p
    }

    /// Create a tresillo pattern (3+3+2)
    pub fn tresillo() -> Self {
        let mut p = Self::new(8);
        p.steps[0].step_type = StepType::Accent;
        p.steps[1].step_type = StepType::Rest;
        p.steps[2].step_type = StepType::Rest;
        p.steps[3].step_type = StepType::Accent;
        p.steps[4].step_type = StepType::Rest;
        p.steps[5].step_type = StepType::Rest;
        p.steps[6].step_type = StepType::Accent;
        p.steps[7].step_type = StepType::Rest;
        p.name = *b"Tresillo\0\0\0\0\0\0\0\0";
        p
    }

    /// Create a syncopated pattern
    pub fn syncopated() -> Self {
        let mut p = Self::new(16);
        let active = [0, 3, 4, 6, 7, 10, 12, 15];
        for i in 0..16 {
            p.steps[i].step_type = if active.contains(&i) {
                if i == 0 || i == 6 || i == 12 {
                    StepType::Accent
                } else {
                    StepType::Play
                }
            } else {
                StepType::Rest
            };
        }
        p.name = *b"Syncopated\0\0\0\0\0\0";
        p
    }

    /// Create a dotted-eighth feel pattern
    pub fn dotted_eighth() -> Self {
        let mut p = Self::new(16);
        let active = [0, 3, 6, 8, 11, 14];
        for i in 0..16 {
            p.steps[i].step_type = if active.contains(&i) {
                StepType::Play
            } else {
                StepType::Rest
            };
        }
        p.steps[0].step_type = StepType::Accent;
        p.steps[8].step_type = StepType::Accent;
        p.name = *b"Dotted 8th\0\0\0\0\0\0";
        p
    }

    /// Create an Afro-Cuban clave pattern (son clave 3-2)
    pub fn son_clave() -> Self {
        let mut p = Self::new(16);
        let hits = [0, 3, 7, 8, 12];
        for i in 0..16 {
            p.steps[i].step_type = if hits.contains(&i) {
                StepType::Accent
            } else {
                StepType::Rest
            };
        }
        p.name = *b"Son Clave 3-2\0\0\0";
        p
    }

    /// Ratchet build-up pattern
    pub fn ratchet_build() -> Self {
        let mut p = Self::new(8);
        p.steps[0].ratchet = 1;
        p.steps[1].ratchet = 1;
        p.steps[2].ratchet = 2;
        p.steps[3].ratchet = 2;
        p.steps[4].ratchet = 2;
        p.steps[5].ratchet = 3;
        p.steps[6].ratchet = 4;
        p.steps[7].ratchet = 4;
        p.name = *b"Ratchet Build\0\0\0";
        p
    }

    /// Helper: 16-step pattern from an active-step list (accents first).
    fn from_hits(active: &[usize], accents: &[usize], name: &[u8; 16]) -> Self {
        let mut p = Self::new(16);
        for i in 0..16 {
            p.steps[i].step_type = if accents.contains(&i) {
                StepType::Accent
            } else if active.contains(&i) {
                StepType::Play
            } else {
                StepType::Rest
            };
        }
        p.name = *name;
        p
    }

    /// Classic uplifting-trance offbeat (chords on the "and" of each beat).
    pub fn trance_offbeat() -> Self {
        Self::from_hits(&[2, 6, 10, 14], &[], b"Trance Offbeat\0\0")
    }

    /// Driving rolling 16ths with accents on the downbeats.
    pub fn rolling_16() -> Self {
        Self::from_hits(&(0..16).collect::<Vec<_>>(), &[0, 4, 8, 12], b"Rolling 16ths\0\0\0")
    }

    /// Gated trance-choir rhythm (the staccato choir-stab feel).
    pub fn trance_gate() -> Self {
        Self::from_hits(&[0, 2, 3, 5, 7, 8, 10, 11, 13, 15], &[0, 8], b"Trance Gate\0\0\0\0\0")
    }

    /// Plucky straight-eighths arp with downbeat accents.
    pub fn trance_pluck() -> Self {
        Self::from_hits(&[0, 2, 4, 6, 8, 10, 12, 14], &[0, 8], b"Trance Pluck\0\0\0\0")
    }

    /// Sparse anthem chord stabs.
    pub fn anthem_stab() -> Self {
        Self::from_hits(&[], &[0, 6, 8, 14], b"Anthem Stab\0\0\0\0\0")
    }

    /// 3-3-3-3-2-2 rolling hat-style 16ths (busy trance arp).
    pub fn rolling_offbeat() -> Self {
        Self::from_hits(&[0, 3, 6, 9, 11, 13, 15], &[0, 9], b"Rolling Offbeat\0")
    }

    pub const PRESET_COUNT: usize = 13;

    pub fn preset(index: usize) -> Self {
        match index {
            0 => Self::new(16),
            1 => Self::four_on_floor(),
            2 => Self::tresillo(),
            3 => Self::syncopated(),
            4 => Self::dotted_eighth(),
            5 => Self::son_clave(),
            6 => Self::ratchet_build(),
            7 => Self::trance_offbeat(),
            8 => Self::rolling_16(),
            9 => Self::trance_gate(),
            10 => Self::trance_pluck(),
            11 => Self::anthem_stab(),
            _ => Self::rolling_offbeat(),
        }
    }

    pub fn preset_name(index: usize) -> &'static str {
        match index {
            0 => "Default",
            1 => "4 on the floor",
            2 => "Tresillo",
            3 => "Syncopated",
            4 => "Dotted 8th",
            5 => "Son Clave 3-2",
            6 => "Ratchet Build",
            7 => "Trance Offbeat",
            8 => "Rolling 16ths",
            9 => "Trance Gate",
            10 => "Trance Pluck",
            11 => "Anthem Stab",
            _ => "Rolling Offbeat",
        }
    }
}

impl Default for RhythmPattern {
    fn default() -> Self {
        Self::new(16)
    }
}
