//! The comb and the allpass the reverb and the delay are built from.

#[cfg(feature = "reverb")]
/// Comb filter for reverb
#[derive(Debug, Clone)]
pub(super) struct CombFilter {
    buffer: Vec<f32>,
    pos: usize,
    feedback: f32,
    filter_state: f32,
    damping: f32,
}

impl CombFilter {
    pub(super) fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            pos: 0,
            feedback: 0.84,
            filter_state: 0.0,
            damping: 0.5,
        }
    }

    pub(super) fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.pos];

        // Apply damping filter
        self.filter_state = output * (1.0 - self.damping) + self.filter_state * self.damping;

        // Write to buffer with NaN/inf guard — prevent runaway feedback
        let new_val = input + self.filter_state * self.feedback;
        self.buffer[self.pos] = if new_val.is_finite() {
            new_val.clamp(-4.0, 4.0)
        } else {
            0.0
        };

        // Advance position
        self.pos = (self.pos + 1) % self.buffer.len();

        if output.is_finite() {
            output
        } else {
            0.0
        }
    }

    pub(super) fn set_feedback(&mut self, feedback: f32) {
        self.feedback = feedback;
    }

    pub(super) fn set_damping(&mut self, damping: f32) {
        self.damping = damping;
    }

    pub(super) fn reset(&mut self) {
        for sample in self.buffer.iter_mut() {
            *sample = 0.0;
        }
        self.pos = 0;
        self.filter_state = 0.0;
    }
}

/// Allpass filter for reverb
#[derive(Debug, Clone)]
pub(super) struct AllPassFilter {
    buffer: Vec<f32>,
    pos: usize,
    feedback: f32,
}

impl AllPassFilter {
    pub(super) fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size.max(1)],
            pos: 0,
            feedback: 0.5,
        }
    }

    pub(super) fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.pos];
        let output = -input + delayed;

        let new_val = input + delayed * self.feedback;
        self.buffer[self.pos] = if new_val.is_finite() {
            new_val.clamp(-4.0, 4.0)
        } else {
            0.0
        };

        self.pos = (self.pos + 1) % self.buffer.len();

        if output.is_finite() {
            output
        } else {
            0.0
        }
    }

    pub(super) fn reset(&mut self) {
        for sample in self.buffer.iter_mut() {
            *sample = 0.0;
        }
        self.pos = 0;
    }
}

//==============================================================================
// CHORUS EFFECT
//==============================================================================
