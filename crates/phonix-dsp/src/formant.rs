//! A vowel as five band-pass filters: the first five formants of a sung
//! vowel, interpolated between five vowels (ah, eh, ee, oh, oo), for a
//! male or a female voice. The frequencies are Peterson and Barney's.

use std::f64::consts::PI;

pub const VOWEL_COUNT: usize = 5;
pub const FORMANT_COUNT: usize = 5;

/// Male formant frequencies, `[vowel][formant]`, F1 to F5.
pub const MALE_FORMANTS: [[f32; FORMANT_COUNT]; VOWEL_COUNT] = [
    [730.0, 1090.0, 2440.0, 3400.0, 4100.0], // ah
    [530.0, 1840.0, 2480.0, 3400.0, 4100.0], // eh
    [270.0, 2290.0, 3010.0, 3300.0, 3850.0], // ee
    [570.0, 840.0, 2410.0, 3400.0, 4100.0],  // oh
    [300.0, 870.0, 2240.0, 3400.0, 4100.0],  // oo
];

/// Female formant frequencies, about 17 percent above the male ones.
pub const FEMALE_FORMANTS: [[f32; FORMANT_COUNT]; VOWEL_COUNT] = [
    [850.0, 1270.0, 2810.0, 3700.0, 4500.0], // ah
    [610.0, 2150.0, 2900.0, 3700.0, 4500.0], // eh
    [310.0, 2680.0, 3520.0, 3800.0, 4600.0], // ee
    [660.0, 980.0, 2820.0, 3700.0, 4500.0],  // oh
    [350.0, 1010.0, 2620.0, 3700.0, 4500.0], // oo
];

pub const FORMANT_Q: [f32; FORMANT_COUNT] = [8.0, 12.0, 15.0, 18.0, 22.0];
pub const FORMANT_GAIN: [f32; FORMANT_COUNT] = [1.0, 0.5, 0.25, 0.12, 0.06];
pub const VOWEL_NAMES: [&str; VOWEL_COUNT] = ["Ah", "Eh", "Ee", "Oh", "Oo"];

/// A band-pass biquad in direct form II transposed, with f64 state.
#[derive(Clone)]
struct BandPass {
    b0: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl BandPass {
    fn new(freq: f32, q: f32, sample_rate: f32) -> Self {
        let mut f = BandPass { b0: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, z1: 0.0, z2: 0.0 };
        f.set(freq, q, sample_rate);
        f
    }

    /// Move the filter to a new centre frequency, keeping what it is
    /// ringing. A vowel that drifts retunes at control rate; rebuilding the
    /// filter instead would clear the resonance every time it moved.
    fn set(&mut self, freq: f32, q: f32, sample_rate: f32) {
        let w0 = 2.0 * PI * freq as f64 / sample_rate as f64;
        let alpha = w0.sin() / (2.0 * q as f64);
        let a0 = 1.0 + alpha;
        self.b0 = alpha / a0;
        self.b2 = -alpha / a0;
        self.a1 = (-2.0 * w0.cos()) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let x = input as f64;
        let y = self.b0 * x + self.z1;
        self.z1 = -self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y as f32
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// Five formant band-passes in parallel.
#[derive(Clone)]
pub struct FormantBank {
    filters: [BandPass; FORMANT_COUNT],
    gains: [f32; FORMANT_COUNT],
    sample_rate: f32,
    tuned_to: (f32, bool),
}

impl FormantBank {
    pub fn new(sample_rate: f32) -> Self {
        let mut bank = FormantBank {
            filters: std::array::from_fn(|_| BandPass::new(1000.0, 1.0, sample_rate)),
            gains: FORMANT_GAIN,
            sample_rate,
            tuned_to: (f32::NAN, false),
        };
        bank.set_vowel(0.0, false);
        bank
    }

    /// `vowel` from 0 (ah) through eh, ee, oh to 1 (oo), interpolated
    /// between neighbours. The filters keep what they are ringing, and a
    /// vowel that has not moved costs nothing.
    pub fn set_vowel(&mut self, vowel: f32, female: bool) {
        let vowel = vowel.clamp(0.0, 1.0);
        if self.tuned_to == (vowel, female) {
            return;
        }
        self.tuned_to = (vowel, female);
        let formants = if female { &FEMALE_FORMANTS } else { &MALE_FORMANTS };
        let pos = vowel * (VOWEL_COUNT - 1) as f32;
        let idx_a = (pos as usize).min(VOWEL_COUNT - 2);
        let idx_b = idx_a + 1;
        let frac = pos - idx_a as f32;
        for f in 0..FORMANT_COUNT {
            let freq = formants[idx_a][f] * (1.0 - frac) + formants[idx_b][f] * frac;
            self.filters[f].set(freq, FORMANT_Q[f], self.sample_rate);
        }
    }

    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let mut out = 0.0;
        for f in 0..FORMANT_COUNT {
            out += self.filters[f].process(input) * self.gains[f];
        }
        out
    }

    pub fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_at(bank: &mut FormantBank, hz: f32, sr: f32) -> f32 {
        bank.reset();
        let n = sr as usize / 2;
        let mut sum = 0.0f32;
        for i in 0..n {
            let x = (i as f32 * hz / sr * std::f32::consts::TAU).sin();
            let y = bank.process(x);
            if i >= n / 2 {
                sum += y * y;
            }
        }
        (sum / (n / 2) as f32).sqrt()
    }

    /// An "ah" passes its first formant and holds back what sits between
    /// formants; an "ee" moves the second formant up by an octave.
    #[test]
    fn the_vowels_sit_where_the_tables_say() {
        let sr = 48_000.0;
        let mut bank = FormantBank::new(sr);
        bank.set_vowel(0.0, false);
        let f1 = level_at(&mut bank, 730.0, sr);
        let between = level_at(&mut bank, 1600.0, sr);
        assert!(f1 > 3.0 * between, "ah: F1 {f1} against {between} between formants");
        bank.set_vowel(0.5, false);
        let ee_f2 = level_at(&mut bank, 2290.0, sr);
        let ee_low = level_at(&mut bank, 1090.0, sr);
        assert!(ee_f2 > 2.0 * ee_low, "ee: F2 {ee_f2} against {ee_low}");
    }

    /// A vowel that has not moved must not disturb what the filters are
    /// ringing: the bank is retuned at control rate, and rebuilding it
    /// there would chop the resonance fifteen hundred times a second.
    #[test]
    fn a_still_vowel_leaves_the_resonance_alone() {
        let sr = 48_000.0;
        let mut bank = FormantBank::new(sr);
        bank.set_vowel(0.2, false);
        let excite = |bank: &mut FormantBank, retune: bool| {
            bank.reset();
            let mut tail = Vec::new();
            for i in 0..2000 {
                let x = if i < 64 { 1.0 } else { 0.0 };
                if retune {
                    bank.set_vowel(0.2, false);
                }
                let y = bank.process(x);
                if i >= 1000 {
                    tail.push(y);
                }
            }
            tail
        };
        let free = excite(&mut bank, false);
        let retuned = excite(&mut bank, true);
        assert_eq!(free, retuned, "retuning to the same vowel changed the ringing");
        assert!(
            free.iter().fold(0.0f32, |a, v| a.max(v.abs())) > 1e-6,
            "the probe left nothing ringing to compare"
        );
    }

    #[test]
    fn a_female_voice_sits_above_a_male_one() {
        for v in 0..VOWEL_COUNT {
            for f in 0..FORMANT_COUNT {
                assert!(FEMALE_FORMANTS[v][f] > MALE_FORMANTS[v][f], "{} F{}", VOWEL_NAMES[v], f + 1);
            }
        }
    }
}
