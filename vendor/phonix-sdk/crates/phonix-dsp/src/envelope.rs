//! Envelope generator implementation for the Phonix synthesizer
//!
//! This module provides ADSR (Attack, Decay, Sustain, Release) envelope generation
//! for controlling amplitude and filter cutoff modulation.

#![allow(dead_code)] // Public API for future use

/// Different stages of an ADSR envelope
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// An ADSR envelope generator
#[derive(Debug, Clone)]
pub struct Envelope {
    /// Current envelope stage
    stage: EnvelopeStage,

    /// Current envelope value (0.0 to 1.0)
    value: f32,

    /// Attack time in seconds
    attack_time: f32,

    /// Decay time in seconds
    decay_time: f32,

    /// Sustain level (0.0 to 1.0)
    sustain_level: f32,

    /// Release time in seconds
    release_time: f32,

    /// Sample rate
    sample_rate: f32,

    /// Attack increment per sample
    attack_increment: f32,

    /// Decay increment per sample
    decay_increment: f32,

    /// Release increment per sample
    release_increment: f32,

    /// Exponential coefficient for decay/release (0 = linear, higher = more exponential)
    exp_coefficient: f32,
}

impl Envelope {
    /// Create a new envelope generator
    pub fn new(sample_rate: f32) -> Self {
        let mut env = Self {
            stage: EnvelopeStage::Idle,
            value: 0.0,
            attack_time: 0.1,   // 100ms default
            decay_time: 0.3,    // 300ms default
            sustain_level: 0.7, // 70% default
            release_time: 0.3,  // 300ms default
            sample_rate,
            attack_increment: 0.0,
            decay_increment: 0.0,
            release_increment: 0.0,
            exp_coefficient: 0.0,
        };
        env.update_increments();
        env
    }

    /// Set exponential curve coefficient (0 = linear, ~5-10 = strongly exponential)
    pub fn set_exp_coefficient(&mut self, coeff: f32) {
        self.exp_coefficient = coeff.max(0.0);
    }

    /// Set the attack time in seconds
    pub fn set_attack(&mut self, attack_time: f32) {
        self.attack_time = attack_time.max(0.001).min(10.0); // Limit to reasonable range
        self.update_increments();
    }

    /// Set the decay time in seconds
    pub fn set_decay(&mut self, decay_time: f32) {
        self.decay_time = decay_time.max(0.001).min(10.0);
        self.update_increments();
    }

    /// Set the sustain level (0.0 to 1.0)
    pub fn set_sustain(&mut self, sustain_level: f32) {
        self.sustain_level = sustain_level.max(0.0).min(1.0);
    }

    /// Set the release time in seconds
    pub fn set_release(&mut self, release_time: f32) {
        self.release_time = release_time.max(0.001).min(10.0);
        self.update_increments();
    }

    /// Trigger the envelope (start attack phase).
    /// Starts from the current value to avoid clicks when retriggering.
    pub fn trigger(&mut self) {
        self.stage = EnvelopeStage::Attack;
        // Don't reset value to 0 — start attack from current level to avoid click
        // when retriggering a voice that's still releasing.
    }

    /// Trigger at peak — skip attack, start directly in decay from value=1.0
    /// Used for pitch envelopes where the sweep must start immediately
    pub fn trigger_at_peak(&mut self) {
        self.stage = EnvelopeStage::Decay;
        self.value = 1.0;
    }

    /// Release the envelope (start release phase)
    pub fn release(&mut self) {
        if self.stage != EnvelopeStage::Idle {
            self.stage = EnvelopeStage::Release;
            // Recalculate release increment based on current value
            // This is crucial because update_increments() may have been called when value was 0
            self.release_increment = -self.value / (self.release_time * self.sample_rate);
        }
    }

    /// Reset the envelope to idle state
    pub fn reset(&mut self) {
        self.stage = EnvelopeStage::Idle;
        self.value = 0.0;
    }

    /// Get the current envelope value
    pub fn value(&self) -> f32 {
        self.value
    }

    /// Get the current envelope stage
    pub fn stage(&self) -> EnvelopeStage {
        self.stage
    }

    /// Process one sample and update the envelope
    pub fn process(&mut self) -> f32 {
        match self.stage {
            EnvelopeStage::Idle => {
                self.value = 0.0;
            }
            EnvelopeStage::Attack => {
                self.value += self.attack_increment;
                if self.value >= 1.0 {
                    self.value = 1.0;
                    self.stage = EnvelopeStage::Decay;
                }
            }
            EnvelopeStage::Decay => {
                if self.exp_coefficient > 0.0 {
                    // Exponential decay: multiply by coefficient each sample
                    let range = self.value - self.sustain_level;
                    let new_range = range * (-self.exp_coefficient / (self.decay_time * self.sample_rate)).exp();
                    self.value = self.sustain_level + new_range;
                    if (self.value - self.sustain_level).abs() < 0.0001 {
                        self.value = self.sustain_level;
                        self.stage = EnvelopeStage::Sustain;
                    }
                } else {
                    self.value += self.decay_increment;
                    if self.value <= self.sustain_level {
                        self.value = self.sustain_level;
                        self.stage = EnvelopeStage::Sustain;
                    }
                }
            }
            EnvelopeStage::Sustain => {
                // In sustain stage, value stays at sustain level
            }
            EnvelopeStage::Release => {
                if self.exp_coefficient > 0.0 {
                    let coeff = (-self.exp_coefficient / (self.release_time * self.sample_rate)).exp();
                    self.value *= coeff;
                    if self.value < 0.0001 {
                        self.value = 0.0;
                        self.stage = EnvelopeStage::Idle;
                    }
                } else {
                    self.value += self.release_increment;
                    if self.value <= 0.0 {
                        self.value = 0.0;
                        self.stage = EnvelopeStage::Idle;
                    }
                }
            }
        }

        self.value
    }

    /// Process multiple samples at once
    pub fn process_block(&mut self, buffer: &mut [f32]) {
        for sample in buffer.iter_mut() {
            *sample = self.process();
        }
    }

    /// Update the increment values based on current times and sample rate
    fn update_increments(&mut self) {
        // Attack: go from 0.0 to 1.0
        self.attack_increment = 1.0 / (self.attack_time * self.sample_rate);

        // Decay: go from 1.0 to sustain_level
        let decay_range = 1.0 - self.sustain_level;
        self.decay_increment = -decay_range / (self.decay_time * self.sample_rate);

        // Release: go from current value to 0.0
        self.release_increment = -self.value / (self.release_time * self.sample_rate);
    }

    /// Set all envelope parameters at once
    pub fn set_parameters(&mut self, attack: f32, decay: f32, sustain: f32, release: f32) {
        self.set_attack(attack);
        self.set_decay(decay);
        self.set_sustain(sustain);
        self.set_release(release);
    }

    /// Check if the envelope is currently active (not idle)
    pub fn is_active(&self) -> bool {
        self.stage != EnvelopeStage::Idle
    }

    /// Check if the envelope is in release phase
    pub fn is_releasing(&self) -> bool {
        self.stage == EnvelopeStage::Release
    }

    /// True only while the gate is held — i.e. Attack / Decay /
    /// Sustain. False during Idle AND during Release.
    ///
    /// Use this (not `is_active()`) for legato / slide / portamento
    /// "is the previous note still being played?" decisions. The
    /// distinction matters because `is_active()` stays true during
    /// the release tail; any glide logic that gates on it will
    /// trigger between back-to-back pattern notes whose NoteOff
    /// landed before the next NoteOn, producing a continuous
    /// glide chain instead of discrete notes.
    pub fn is_held(&self) -> bool {
        matches!(
            self.stage,
            EnvelopeStage::Attack | EnvelopeStage::Decay | EnvelopeStage::Sustain
        )
    }
}

/// A specialized envelope for filter modulation
#[derive(Debug, Clone)]
pub struct FilterEnvelope {
    /// Base envelope generator
    envelope: Envelope,

    /// Amount of envelope modulation (typically -1.0 to 1.0)
    modulation_amount: f32,

    /// Base cutoff frequency when envelope is at 0
    base_cutoff: f32,
}

impl FilterEnvelope {
    /// Create a new filter envelope
    pub fn new(sample_rate: f32) -> Self {
        Self {
            envelope: Envelope::new(sample_rate),
            modulation_amount: 0.5, // Default moderate modulation
            base_cutoff: 1000.0,    // Default base cutoff
        }
    }

    /// Set the modulation amount
    pub fn set_modulation_amount(&mut self, amount: f32) {
        self.modulation_amount = amount.max(-1.0).min(1.0);
    }

    /// Set the base cutoff frequency
    pub fn set_base_cutoff(&mut self, cutoff: f32) {
        self.base_cutoff = cutoff.max(20.0).min(20000.0);
    }

    /// Trigger the envelope
    pub fn trigger(&mut self) {
        self.envelope.trigger();
    }

    /// Release the envelope
    pub fn release(&mut self) {
        self.envelope.release();
    }

    /// Reset the envelope
    pub fn reset(&mut self) {
        self.envelope.reset();
    }

    /// Process one sample and get the modulated cutoff frequency
    pub fn process(&mut self) -> f32 {
        let env_value = self.envelope.process();

        // Calculate modulated cutoff
        // Base cutoff * (1.0 + modulation_amount * envelope_value)
        let modulation = 1.0 + (self.modulation_amount * env_value);
        self.base_cutoff * modulation.max(0.01) // Ensure we don't go to zero
    }

    /// Get the current envelope value
    pub fn envelope_value(&self) -> f32 {
        self.envelope.value()
    }

    /// Set all parameters at once
    pub fn set_parameters(
        &mut self,
        attack: f32,
        decay: f32,
        sustain: f32,
        release: f32,
        modulation: f32,
        base_cutoff: f32,
    ) {
        self.envelope
            .set_parameters(attack, decay, sustain, release);
        self.set_modulation_amount(modulation);
        self.set_base_cutoff(base_cutoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_envelope_creation() {
        let sample_rate = 44100.0;
        let env = Envelope::new(sample_rate);

        assert_eq!(env.stage(), EnvelopeStage::Idle);
        assert_eq!(env.value(), 0.0);
        assert_eq!(env.is_active(), false);
    }

    #[test]
    fn test_envelope_trigger() {
        let sample_rate = 44100.0;
        let mut env = Envelope::new(sample_rate);

        env.trigger();
        assert_eq!(env.stage(), EnvelopeStage::Attack);
        assert_eq!(env.value(), 0.0);

        // Process a few samples
        let val1 = env.process();
        let val2 = env.process();

        assert!(val1 > 0.0);
        assert!(val2 > val1); // Should be increasing during attack
    }

    #[test]
    fn test_envelope_release() {
        let sample_rate = 44100.0;
        let mut env = Envelope::new(sample_rate);

        env.trigger();

        // Fast forward to sustain stage
        env.set_attack(0.001);
        env.set_decay(0.001);

        // Process until sustain
        while env.stage() != EnvelopeStage::Sustain {
            env.process();
        }

        assert_eq!(env.stage(), EnvelopeStage::Sustain);

        env.release();
        assert_eq!(env.stage(), EnvelopeStage::Release);
    }

    #[test]
    fn test_filter_envelope() {
        let sample_rate = 44100.0;
        let mut filter_env = FilterEnvelope::new(sample_rate);

        filter_env.set_base_cutoff(1000.0);
        filter_env.set_modulation_amount(0.5);

        let cutoff = filter_env.process();
        assert!(cutoff >= 1000.0); // Should be at least base cutoff
    }

    /// `is_held()` must return false when the envelope is in Idle
    /// OR Release — only Attack/Decay/Sustain count. This is the
    /// contract every legato / portamento / slide check across
    /// the codebase relies on.
    #[test]
    fn envelope_is_held_excludes_release_and_idle() {
        let sr = 48_000.0;
        let mut env = Envelope::new(sr);
        // Slow attack so we stay in Attack long enough to inspect.
        env.set_parameters(0.5, 0.1, 0.7, 0.5);
        assert!(!env.is_held(), "idle env must not be held");
        assert!(!env.is_active(), "idle env must not be active");

        env.trigger();
        env.process(); // step into Attack
        assert!(env.is_held(), "Attack stage must count as held");
        assert!(env.is_active());

        // Run forward enough to reach Sustain.
        for _ in 0..(sr as usize) { let _ = env.process(); }
        assert!(env.is_held(), "Sustain stage must count as held");

        // Release — held=false but active=true (the tail).
        env.release();
        env.process();
        assert!(!env.is_held(), "Release stage must NOT count as held");
        assert!(env.is_active(), "Release stage must still be active");
        assert!(env.is_releasing());

        // Let it die out.
        for _ in 0..(2 * sr as usize) { let _ = env.process(); }
        assert!(!env.is_held());
        assert!(!env.is_active());
    }
}
