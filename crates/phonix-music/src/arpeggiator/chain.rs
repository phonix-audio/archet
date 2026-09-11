//! Pattern chain — sequence multiple rhythm patterns for longer phrases.

use super::step::RhythmPattern;

/// Maximum patterns in the bank
pub const MAX_PATTERNS: usize = 8;
/// Maximum slots in a chain
pub const MAX_CHAIN_SLOTS: usize = 16;

/// A slot in the pattern chain: which pattern to play and how many times
#[derive(Debug, Clone, Copy)]
pub struct ChainSlot {
    /// Index into the pattern bank (0–7)
    pub pattern_index: u8,
    /// How many times to repeat this pattern before advancing (1–16)
    pub repeat_count: u8,
}

impl Default for ChainSlot {
    fn default() -> Self {
        Self {
            pattern_index: 0,
            repeat_count: 1,
        }
    }
}

/// Pattern chain: sequences multiple rhythm patterns.
/// Example: [(0, 2), (1, 1), (0, 1)] plays pattern A twice, B once, A once, then loops.
#[derive(Debug, Clone)]
pub struct PatternChain {
    /// Pattern bank (up to 8 rhythm patterns)
    pub patterns: Vec<RhythmPattern>,
    /// Chain sequence
    pub slots: Vec<ChainSlot>,
    /// Whether chain mode is active (false = use single pattern)
    pub enabled: bool,

    // Playback state
    current_slot: usize,
    current_repeat: u8,
}

impl PatternChain {
    pub fn new() -> Self {
        Self {
            patterns: vec![RhythmPattern::default()],
            slots: vec![ChainSlot::default()],
            enabled: false,
            current_slot: 0,
            current_repeat: 0,
        }
    }

    /// Get the currently active rhythm pattern
    pub fn current_pattern(&self) -> &RhythmPattern {
        if !self.enabled || self.slots.is_empty() || self.patterns.is_empty() {
            return self.patterns.first().unwrap_or(&DEFAULT_PATTERN);
        }
        let slot = &self.slots[self.current_slot % self.slots.len()];
        let idx = slot.pattern_index as usize % self.patterns.len();
        &self.patterns[idx]
    }

    /// Advance: called when the current pattern finishes one cycle.
    /// Returns true if the chain wrapped back to the start.
    pub fn advance(&mut self) -> bool {
        if !self.enabled || self.slots.is_empty() {
            return false;
        }

        self.current_repeat += 1;
        let slot = &self.slots[self.current_slot % self.slots.len()];
        if self.current_repeat >= slot.repeat_count {
            self.current_repeat = 0;
            self.current_slot += 1;
            if self.current_slot >= self.slots.len() {
                self.current_slot = 0;
                return true; // chain wrapped
            }
        }
        false
    }

    /// Reset chain playback to start
    pub fn reset(&mut self) {
        self.current_slot = 0;
        self.current_repeat = 0;
    }

    /// Add a pattern to the bank. Returns its index.
    pub fn add_pattern(&mut self, pattern: RhythmPattern) -> usize {
        if self.patterns.len() < MAX_PATTERNS {
            self.patterns.push(pattern);
        }
        self.patterns.len() - 1
    }

    /// Add a slot to the chain
    pub fn add_slot(&mut self, pattern_index: u8, repeat_count: u8) {
        if self.slots.len() < MAX_CHAIN_SLOTS {
            self.slots.push(ChainSlot {
                pattern_index,
                repeat_count: repeat_count.max(1),
            });
        }
    }
}

static DEFAULT_PATTERN: RhythmPattern = RhythmPattern {
    steps: [super::step::StepData {
        step_type: super::step::StepType::Play,
        velocity: 0,
        gate_length: 0.0,
        probability: 1.0,
        ratchet: 1,
    }; super::step::MAX_STEPS],
    step_count: 16,
    name: *b"Default\0\0\0\0\0\0\0\0\0",
};
