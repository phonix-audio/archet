//! Simple ADSR envelope generator with exponential curves.

#[derive(Clone, Copy, PartialEq)]
enum Stage { Idle, Attack, Decay, Sustain, Release }

pub struct Adsr {
    stage:   Stage,
    level:   f32,
    inv_sr:  f32,
    // Coefficients: per-sample multiplicative (exponential)
    a_rate:  f32,   // attack: additive rate per sample
    d_coef:  f32,   // decay:  exp(-1 / (decay * sr))
    sustain: f32,
    r_coef:  f32,   // release: exp(-1 / (release * sr))
}

impl Adsr {
    pub fn new(sr: f32) -> Self {
        let mut s = Self {
            stage: Stage::Idle, level: 0.0, inv_sr: 1.0 / sr,
            a_rate: 0.0, d_coef: 0.0, sustain: 0.7, r_coef: 0.0,
        };
        s.set_params(0.01, 0.1, 0.7, 0.3);
        s
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.inv_sr = 1.0 / sr;
    }

    pub fn set_params(&mut self, attack: f32, decay: f32, sustain: f32, release: f32) {
        let sr = 1.0 / self.inv_sr;
        self.a_rate  = self.inv_sr / attack.max(0.001);
        self.d_coef  = (-1.0 / (decay.max(0.001) * sr)).exp();
        self.sustain = sustain.clamp(0.0, 1.0);
        self.r_coef  = (-1.0 / (release.max(0.005) * sr)).exp();
    }

    pub fn note_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub fn note_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    /// True only while the gate is HELD (Attack / Decay / Sustain), false in
    /// Idle and in Release.
    ///
    /// Use this, never `is_active`, for legato / glide / portamento decisions.
    /// `is_active` stays true through the release tail, so any glide gated on
    /// it fires between notes whose note-off landed before the next note-on,
    /// and you get one continuous slide instead of separate notes.
    ///
    /// The same name and the same warning are on `crate::envelope::Envelope`.
    pub fn is_held(&self) -> bool {
        matches!(self.stage, Stage::Attack | Stage::Decay | Stage::Sustain)
    }

    #[inline(always)]
    pub fn process(&mut self) -> f32 {
        match self.stage {
            Stage::Idle => { self.level = 0.0; }
            Stage::Attack => {
                self.level += self.a_rate;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level = self.sustain + (self.level - self.sustain) * self.d_coef;
                if (self.level - self.sustain).abs() < 1e-4 {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {
                self.level = self.sustain;
            }
            Stage::Release => {
                self.level *= self.r_coef;
                if self.level < 1e-5 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }

    pub fn is_active(&self) -> bool {
        self.stage != Stage::Idle
    }

    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }
}

#[cfg(test)]
mod held_tests {
    use super::*;

    /// `is_held` must be false during the release, which is the whole point:
    /// `is_active` is true there and that is what caused the glide bug.
    #[test]
    fn is_held_is_false_once_released() {
        let sr = 48_000.0;
        let mut e = Adsr::new(sr);
        e.set_params(0.001, 0.01, 0.8, 1.0);
        e.note_on();
        for _ in 0..1000 { let _ = e.process(); }
        assert!(e.is_held(), "a sounding, un-released note must read as held");

        e.note_off();
        let _ = e.process();
        assert!(!e.is_held(), "a released note must NOT read as held");
        assert!(e.is_active(), "...while still being active, which is the trap");
    }
}
