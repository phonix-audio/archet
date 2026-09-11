//! Per-note velocity dynamics for singing voices: a natural diminuendo after
//! the attack. A singer eases off a hard-struck note rather than holding a dead
//! constant level; the harder the onset (velocity), the more it settles back.
//! Soft notes barely move (they were already gentle). Shared across voice
//! engines so the behaviour is consistent.

/// One voice's diminuendo envelope. Reset at each fresh attack with
/// [`VoiceDynamics::note_on`], advance per sample with [`VoiceDynamics::step`].
#[derive(Debug, Clone, Copy)]
pub struct VoiceDynamics {
    /// Current level multiplier, 1.0 at attack, easing toward the velocity-set
    /// floor.
    dyn_env: f32,
    /// One-pole coefficient toward the floor, from the time constant + sr.
    coef: f32,
}

impl VoiceDynamics {
    /// `secs` is the diminuendo time constant (~1.0-1.5 s for a solo voice;
    /// shorter = a faster settle).
    pub fn new(sample_rate: f32, secs: f32) -> Self {
        Self {
            dyn_env: 1.0,
            coef: 1.0 - (-1.0 / (secs.max(0.01) * sample_rate)).exp(),
        }
    }

    /// Restart the diminuendo at full level (call on a fresh note-on, NOT on a
    /// legato slur — a slur keeps the current settle).
    pub fn note_on(&mut self) {
        self.dyn_env = 1.0;
    }

    /// Advance one sample and return the level multiplier (<= 1.0).
    ///
    /// `velocity` in 0..1; `floor_amount` how far a full-velocity note settles
    /// (e.g. 0.25 = a loud note eases to 0.75, a soft note stays ~1.0). The
    /// floor tracks the current velocity so it can be read live from the patch.
    #[inline]
    pub fn step(&mut self, velocity: f32, floor_amount: f32) -> f32 {
        let floor = 1.0 - velocity.clamp(0.0, 1.0) * floor_amount.clamp(0.0, 1.0);
        self.dyn_env += (floor - self.dyn_env) * self.coef;
        self.dyn_env
    }
}

/// Velocity -> level, gentle so soft notes stay audible rather than vanishing:
/// `0.45 + 0.55 * vel`. A drop-in for a bare `* velocity` that ignores how
/// unmusical a near-silent soft note is.
#[inline]
pub fn velocity_to_level(velocity: f32) -> f32 {
    0.45 + 0.55 * velocity.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diminuendo_settles_toward_velocity_floor() {
        let sr = 48_000.0;
        // Loud note: settles well below 1.0; soft note: barely moves.
        let mut loud = VoiceDynamics::new(sr, 1.3);
        let mut soft = VoiceDynamics::new(sr, 1.3);
        loud.note_on();
        soft.note_on();
        let (mut lv, mut sv) = (1.0, 1.0);
        // ~5 s: well past the 1.3 s time constant so it has fully settled.
        for _ in 0..(sr as usize * 5) {
            lv = loud.step(1.0, 0.5);
            sv = soft.step(0.0, 0.5);
        }
        assert!((lv - 0.5).abs() < 0.02, "loud note settles near 0.5, got {lv}");
        assert!(sv > 0.99, "soft note barely diminuendos, got {sv}");
    }

    #[test]
    fn velocity_to_level_keeps_soft_notes_audible() {
        assert!((velocity_to_level(0.0) - 0.45).abs() < 1e-6);
        assert!((velocity_to_level(1.0) - 1.0).abs() < 1e-6);
    }
}
