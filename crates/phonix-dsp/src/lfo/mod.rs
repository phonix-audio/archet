//! Shared low-frequency oscillator.
//!
//! Multi-waveform LFO (sine / triangle / saw / square / sample-and-hold /
//! smoothed random) with a cached last value so a mod matrix can read it at
//! control rate without advancing the phase twice.
//!
//! The generator is engine-neutral; the *destination* of the modulation is not,
//! so it stays with each engine. This type only carries an `enabled` gate:
//! set it from `(destination != Off)` and route the value yourself.

use std::f32::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoWaveform {
    Sine,
    Triangle,
    Saw,
    Square,
    SampleHold,
    Random,
}

impl LfoWaveform {
    pub const ALL: &'static [LfoWaveform] = &[
        LfoWaveform::Sine,
        LfoWaveform::Triangle,
        LfoWaveform::Saw,
        LfoWaveform::Square,
        LfoWaveform::SampleHold,
        LfoWaveform::Random,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            LfoWaveform::Sine => "Sine",
            LfoWaveform::Triangle => "Tri",
            LfoWaveform::Saw => "Saw",
            LfoWaveform::Square => "Sqr",
            LfoWaveform::SampleHold => "S&H",
            LfoWaveform::Random => "Rnd",
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => LfoWaveform::Sine,
            1 => LfoWaveform::Triangle,
            2 => LfoWaveform::Saw,
            3 => LfoWaveform::Square,
            4 => LfoWaveform::SampleHold,
            5 => LfoWaveform::Random,
            _ => LfoWaveform::Sine,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            LfoWaveform::Sine => 0,
            LfoWaveform::Triangle => 1,
            LfoWaveform::Saw => 2,
            LfoWaveform::Square => 3,
            LfoWaveform::SampleHold => 4,
            LfoWaveform::Random => 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Lfo {
    pub rate: f32,
    pub depth: f32,
    pub waveform: LfoWaveform,
    /// Gate: when false, `tick()` returns 0 and does not advance. Engines set
    /// this from their own `destination != Off`.
    pub enabled: bool,
    pub key_sync: bool,

    phase: f32,
    sh_value: f32,
    sh_rng: u32,
    // Random waveform: smoothed interpolation between targets
    random_current: f32,
    random_target: f32,
    sample_rate: f32,
    /// Last value returned by `tick()` — exposed so the mod matrix
    /// can read the LFO at control rate without advancing phase.
    last_value: f32,
}

impl Lfo {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            rate: 1.0,
            depth: 0.0,
            waveform: LfoWaveform::Sine,
            enabled: false,
            key_sync: false,
            phase: 0.0,
            sh_value: 0.0,
            sh_rng: 0x1234_ABCD,
            random_current: 0.0,
            random_target: 0.0,
            sample_rate,
            last_value: 0.0,
        }
    }

    /// Read the last computed LFO value (the cached output from the
    /// most recent `tick()` call). Used by the mod matrix at control
    /// rate so it doesn't advance the LFO phase a second time.
    #[inline(always)]
    pub fn current_value(&self) -> f32 { self.last_value }

    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    #[inline(always)]
    fn next_rand(&mut self) -> f32 {
        self.sh_rng ^= self.sh_rng << 13;
        self.sh_rng ^= self.sh_rng >> 17;
        self.sh_rng ^= self.sh_rng << 5;
        (self.sh_rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Advance LFO and return value in -1..1 (scaled by depth)
    pub fn tick(&mut self) -> f32 {
        if !self.enabled || self.depth < 0.001 {
            self.last_value = 0.0;
            return 0.0;
        }

        let inc = self.rate / self.sample_rate;
        self.phase += inc;
        let wrapped = self.phase >= 1.0;
        if wrapped {
            self.phase -= 1.0;
            // Update S&H / Random target on phase wrap
            self.sh_value = self.next_rand();
            self.random_target = self.next_rand();
        }

        let raw = match self.waveform {
            LfoWaveform::Sine => {
                (self.phase * TAU).sin()
            }
            LfoWaveform::Triangle => {
                let p = self.phase;
                if p < 0.25 { p * 4.0 }
                else if p < 0.75 { 2.0 - p * 4.0 }
                else { p * 4.0 - 4.0 }
            }
            LfoWaveform::Saw => {
                2.0 * self.phase - 1.0
            }
            LfoWaveform::Square => {
                if self.phase < 0.5 { 1.0 } else { -1.0 }
            }
            LfoWaveform::SampleHold => {
                self.sh_value
            }
            LfoWaveform::Random => {
                // Smoothly interpolate toward random target
                let alpha = (inc * 4.0).min(1.0);
                self.random_current += (self.random_target - self.random_current) * alpha;
                self.random_current
            }
        };

        let out = raw * self.depth;
        self.last_value = out;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every 250th sample of each waveform (rate 200, depth 0.8), captured
    /// from the hand-rolled LFO this one replaced.
    #[test]
    fn renders_what_the_legacy_lfo_rendered() {
        let golden: [[f32; 16]; 6] = [
            [0.022793053, 0.6109959, 0.7922953, 0.44595227, -0.1973795, -0.70926344, -0.7488025, -0.28966483, 0.36237988, 0.7730921, 0.66894984, 0.11931024, -0.5097863, -0.7993824, -0.55661774, 0.056835294],
            [0.014512472, 0.44262624, 0.7292596, 0.30114642, -0.12696667, -0.5550799, -0.61680716, -0.188694, 0.23941961, 0.66753376, 0.5043527, 0.07623968, -0.3518736, -0.77998656, -0.39190027, 0.036212977],
            [-0.7927438, -0.5786869, -0.3646298, -0.15057321, 0.063483335, 0.27753994, 0.49159643, 0.705653, -0.6802902, -0.46623316, -0.25217634, -0.03811984, 0.1759368, 0.38999328, 0.60404986, -0.7818935],
            [0.8, 0.8, 0.8, 0.8, -0.8, -0.8, -0.8, -0.8, 0.8, 0.8, 0.8, 0.8, -0.8, -0.8, -0.8, 0.8],
            [0.0, -0.106927015, -0.3166637, 0.59056646, -0.28080773, -0.74513, -0.07422328, 0.5604243, -0.67608017, 0.05129204, 0.44026357, -0.55317444, 0.6323209, 0.7377004, 0.29205018, 0.13862896],
            [0.0, -0.21941598, -0.051966, 0.41264302, -0.6134316, -0.63989997, -0.31496286, -0.20853432, -0.75387603, -0.15678486, 0.54649246, 0.2589636, -0.29868966, 0.575902, -0.29782686, 0.38601837],
        ];
        for (wf, want) in golden.iter().enumerate() {
            let mut lfo = Lfo::new(44100.0);
            lfo.waveform = LfoWaveform::from_index(wf);
            lfo.enabled = true;
            lfo.rate = 200.0;
            lfo.depth = 0.8;
            let got: Vec<f32> = (0..4000).map(|_| lfo.tick()).step_by(250).collect();
            assert_close(&got, want, &format!("LFO waveform {wf}"));
        }
    }

    /// Within 1e-5 of a rendering captured on another machine: libm and FMA
    /// contraction move the last bits, an algorithm change moves far more.
    fn assert_close(got: &[f32], want: &[f32], what: &str) {
        assert_eq!(got.len(), want.len(), "{what}: length");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!((g - w).abs() <= 1e-5, "{what} drifted at sample {i}: {g} vs {w}");
        }
    }

}

pub mod vintage;
