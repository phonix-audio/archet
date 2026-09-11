//! Real-time pitch shifter using dual-head granular delay line.
//!
//! Optimized for zero transcendental math in the per-sample path.
//! Uses precomputed Hann crossfade table instead of sin().

const GRAIN_SIZE: usize = 1024;
/// Samples between a sample entering the shifter and it leaving, shifted.
pub const LATENCY_SAMPLES: usize = GRAIN_SIZE;
const BUFFER_SIZE: usize = 8192;
const BUFFER_MASK: usize = BUFFER_SIZE - 1;

/// Precomputed Hann² crossfade table (256 entries for grain phase 0..1).
const XFADE_TABLE_SIZE: usize = 256;

fn build_xfade_table() -> [f32; XFADE_TABLE_SIZE] {
    let mut table = [0.0f32; XFADE_TABLE_SIZE];
    let mut i = 0;
    while i < XFADE_TABLE_SIZE {
        let phase = i as f64 / XFADE_TABLE_SIZE as f64;
        let s = (std::f64::consts::PI * phase).sin();
        table[i] = (s * s) as f32;
        i += 1;
    }
    table
}

/// Lightweight real-time pitch shifter — zero sin()/cos() in hot path.
pub struct PitchShifter {
    buffer: [f32; BUFFER_SIZE],
    write_pos: usize,
    delay_a: f32,
    delay_b: f32,
    grain_size: f32,
    xfade_table: [f32; XFADE_TABLE_SIZE],
}

impl PitchShifter {
    pub fn new(_sample_rate: f32) -> Self {
        Self {
            buffer: [0.0; BUFFER_SIZE],
            write_pos: 0,
            delay_a: 0.0,
            delay_b: GRAIN_SIZE as f32 * 0.5,
            grain_size: GRAIN_SIZE as f32,
            xfade_table: build_xfade_table(),
        }
    }

    pub fn with_size(sample_rate: f32, _fft_size: usize, _hop_size: usize) -> Self {
        Self::new(sample_rate)
    }

    #[inline]
    pub fn process(&mut self, input: f32, pitch_ratio: f32, _formant_shift: f32) -> f32 {
        self.buffer[self.write_pos] = input;

        // Advance delays
        let sweep = 1.0 - pitch_ratio;
        self.delay_a = fast_rem_euclid(self.delay_a + sweep, self.grain_size);
        self.delay_b = fast_rem_euclid(self.delay_b + sweep, self.grain_size);

        // Read with interpolation
        let out_a = self.read_at_delay(self.delay_a);
        let out_b = self.read_at_delay(self.delay_b);

        // Crossfade via lookup table (no sin())
        let inv_grain = 1.0 / self.grain_size;
        let win_a = self.xfade_lookup(self.delay_a * inv_grain);
        let win_b = self.xfade_lookup(self.delay_b * inv_grain);

        let win_sum = win_a + win_b;
        let output = if win_sum > 1e-6 {
            (out_a * win_a + out_b * win_b) / win_sum
        } else {
            (out_a + out_b) * 0.5
        };

        self.write_pos = (self.write_pos + 1) & BUFFER_MASK;
        output
    }

    pub fn latency(&self) -> usize { GRAIN_SIZE }

    pub fn reset(&mut self) {
        self.buffer = [0.0; BUFFER_SIZE];
        self.write_pos = 0;
        self.delay_a = 0.0;
        self.delay_b = GRAIN_SIZE as f32 * 0.5;
    }

    #[inline(always)]
    fn read_at_delay(&self, delay: f32) -> f32 {
        let read_pos = (self.write_pos as f32 - delay).rem_euclid(BUFFER_SIZE as f32);
        let idx0 = read_pos as usize & BUFFER_MASK;
        let idx1 = (idx0 + 1) & BUFFER_MASK;
        let frac = read_pos - read_pos.floor();
        self.buffer[idx0] + (self.buffer[idx1] - self.buffer[idx0]) * frac
    }

    /// Table lookup with linear interpolation. phase in [0, 1).
    #[inline(always)]
    fn xfade_lookup(&self, phase: f32) -> f32 {
        let p = phase * XFADE_TABLE_SIZE as f32;
        let idx = p as usize;
        if idx >= XFADE_TABLE_SIZE - 1 {
            return self.xfade_table[XFADE_TABLE_SIZE - 1];
        }
        let frac = p - idx as f32;
        self.xfade_table[idx] + (self.xfade_table[idx + 1] - self.xfade_table[idx]) * frac
    }
}

/// Fast rem_euclid for positive modulus. Avoids branching for common case.
#[inline(always)]
fn fast_rem_euclid(val: f32, modulus: f32) -> f32 {
    let r = val % modulus;
    if r < 0.0 { r + modulus } else { r }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    #[ignore]
    fn dump_shift_warble() {
        let sr = 48000.0;
        for (ratio, path) in [(1.5f32, "/tmp/pshift_fifth.wav"), (2.0, "/tmp/pshift_octave.wav")] {
            let mut shifter = PitchShifter::new(sr);
            let mut out = Vec::new();
            for i in 0..(2.0 * sr) as usize {
                let input = (2.0 * PI * 220.0 * i as f32 / sr).sin() * 0.5;
                out.push(shifter.process(input, ratio, 1.0));
            }
            let spec = hound::WavSpec { channels: 1, sample_rate: sr as u32,
                bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
            let mut w = hound::WavWriter::create(path, spec).unwrap();
            for &s in &out { w.write_sample(s).unwrap(); }
            w.finalize().unwrap();
            eprintln!("{path} dumped");
        }
    }

    #[test]
    fn test_unity_passthrough() {
        let sr = 48000.0;
        let mut shifter = PitchShifter::new(sr);
        let freq = 440.0;
        let mut output = Vec::new();
        for i in 0..16384 {
            let input = (2.0 * PI * freq * i as f32 / sr).sin() * 0.5;
            output.push(shifter.process(input, 1.0, 1.0));
        }
        let tail = &output[2048..];
        let rms: f32 = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.2, "Unity RMS = {}", rms);
    }

    #[test]
    fn test_octave_up_has_signal() {
        let sr = 48000.0;
        let mut shifter = PitchShifter::new(sr);
        let mut output = Vec::new();
        for i in 0..16384 {
            let input = (2.0 * PI * 220.0 * i as f32 / sr).sin() * 0.5;
            output.push(shifter.process(input, 2.0, 1.0));
        }
        let tail = &output[2048..];
        let rms: f32 = (tail.iter().map(|x| x * x).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.1, "Octave up RMS = {}", rms);
    }

    #[test]
    fn test_pitch_shift_detectable() {
        use crate::pitch::yin::YinDetector;
        let sr = 48000.0;
        let mut shifter = PitchShifter::new(sr);
        let mut detector = YinDetector::default_for_sample_rate(sr);
        let mut output = Vec::new();
        for i in 0..24000 {
            let input = (2.0 * PI * 220.0 * i as f32 / sr).sin() * 0.5;
            output.push(shifter.process(input, 2.0, 1.0));
        }
        for &s in &output[4096..] { detector.push_sample(s); }
        let detected = detector.pitch();
        assert!(detected > 0.0, "No pitch detected");
        let error_cents = 1200.0 * (detected / 440.0).log2().abs();
        assert!(error_cents < 200.0, "Expected ~440Hz, got {}Hz ({:.0}ct off)", detected, error_cents);
    }

    #[test]
    fn test_fifth_up() {
        use crate::pitch::yin::YinDetector;
        let sr = 48000.0;
        let ratio = 2.0f32.powf(7.0 / 12.0);
        let mut shifter = PitchShifter::new(sr);
        let mut detector = YinDetector::default_for_sample_rate(sr);
        let mut output = Vec::new();
        for i in 0..24000 {
            let input = (2.0 * PI * 262.0 * i as f32 / sr).sin() * 0.5;
            output.push(shifter.process(input, ratio, 1.0));
        }
        for &s in &output[4096..] { detector.push_sample(s); }
        let detected = detector.pitch();
        let expected = 262.0 * ratio;
        let error_cents = 1200.0 * (detected / expected).log2().abs();
        assert!(error_cents < 200.0, "Expected ~{}Hz, got {}Hz ({:.0}ct off)", expected, detected, error_cents);
    }
}
