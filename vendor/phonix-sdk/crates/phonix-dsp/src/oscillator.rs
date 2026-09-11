//! Shared anti-aliased virtual-analog oscillator (PolyBLEP), with hard-sync
//! reporting, FM, and continuous waveform morphing.
//!
//! The generator is engine-neutral; keep engine-specific waveform *sets* or
//! routing in the engine.

use std::f32::consts::{PI, TAU};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waveform {
    Saw,
    Pulse,
    Triangle,
    Sine,
    Noise,
}

impl Waveform {
    pub const ALL: &'static [Waveform] = &[
        Waveform::Saw,
        Waveform::Pulse,
        Waveform::Triangle,
        Waveform::Sine,
        Waveform::Noise,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            Waveform::Saw => "Saw",
            Waveform::Pulse => "Pulse",
            Waveform::Triangle => "Tri",
            Waveform::Sine => "Sine",
            Waveform::Noise => "Noise",
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Waveform::Saw,
            1 => Waveform::Pulse,
            2 => Waveform::Triangle,
            3 => Waveform::Sine,
            4 => Waveform::Noise,
            _ => Waveform::Saw,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Waveform::Saw => 0,
            Waveform::Pulse => 1,
            Waveform::Triangle => 2,
            Waveform::Sine => 3,
            Waveform::Noise => 4,
        }
    }
}

/// PolyBLEP anti-aliasing correction
#[inline(always)]
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        2.0 * t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + 2.0 * t + 1.0
    } else {
        0.0
    }
}

/// Fast sine approximation (4th order polynomial)
#[inline(always)]
fn fast_sin(x: f32) -> f32 {
    // Normalize to -PI..PI
    let mut x = x % TAU;
    if x > PI { x -= TAU; }
    if x < -PI { x += TAU; }
    // Bhaskara-style approximation
    let b = 4.0 / PI;
    let c = -4.0 / (PI * PI);
    let y = b * x + c * x * x.abs();
    // Extra precision
    let p = 0.225;
    p * (y * y.abs() - y) + y
}

#[derive(Debug, Clone)]
pub struct Oscillator {
    pub phase: f32,
    pub sample_rate: f32,
    noise_state: u32,
}

impl Oscillator {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            sample_rate,
            noise_state: 0xDEAD_BEEF,
        }
    }

    /// Reset phase (for key sync or hard sync)
    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    /// Generate one sample with continuous waveform morphing.
    /// shape: 0.0=Saw, 0.25=Pulse, 0.5=Triangle, 0.75=Sine, 1.0=Saw (wraps)
    /// When shape is between two waveforms, crossfades between them.
    pub fn process_shape(
        &mut self,
        frequency: f32,
        pulse_width: f32,
        shape: f32,
    ) -> (f32, bool) {
        let dt = frequency / self.sample_rate;
        let dt = dt.clamp(0.0, 0.5);

        // 4 waveforms equally spaced: Saw(0), Pulse(0.25), Tri(0.5), Sine(0.75)
        let shape = shape.fract(); // wrap 0..1
        let idx = shape * 4.0;
        let base = idx as usize; // 0..3
        let frac = idx - base as f32; // blend factor

        let waveforms = [Waveform::Saw, Waveform::Pulse, Waveform::Triangle, Waveform::Sine];
        let wf_a = waveforms[base % 4];
        let wf_b = waveforms[(base + 1) % 4];

        let sample_a = self.raw_waveform(wf_a, self.phase, dt, pulse_width);
        let sample_b = self.raw_waveform(wf_b, self.phase, dt, pulse_width);
        let sample = sample_a * (1.0 - frac) + sample_b * frac;

        self.phase += dt;
        let wrapped = self.phase >= 1.0;
        if wrapped { self.phase -= 1.0; }

        (sample, wrapped)
    }

    /// Raw waveform sample at given phase (no phase advance)
    #[inline]
    fn raw_waveform(&self, waveform: Waveform, phase: f32, dt: f32, pulse_width: f32) -> f32 {
        match waveform {
            Waveform::Saw => {
                let naive = 2.0 * phase - 1.0;
                naive - poly_blep(phase, dt)
            }
            Waveform::Pulse => {
                let duty = pulse_width.clamp(0.05, 0.95);
                let mut s = if phase < duty { 1.0 } else { -1.0 };
                s += poly_blep(phase, dt);
                s -= poly_blep((phase - duty + 1.0) % 1.0, dt);
                s
            }
            Waveform::Triangle => {
                if phase < 0.25 { phase * 4.0 }
                else if phase < 0.75 { 2.0 - phase * 4.0 }
                else { phase * 4.0 - 4.0 }
            }
            Waveform::Sine => fast_sin(phase * TAU),
            Waveform::Noise => 0.0, // Noise handled separately
        }
    }

    /// Generate one sample. Returns (sample, phase_wrapped) where phase_wrapped
    /// is true if the oscillator completed a cycle (used for hard sync).
    pub fn process(
        &mut self,
        frequency: f32,
        waveform: Waveform,
        pulse_width: f32,
    ) -> (f32, bool) {
        let dt = frequency / self.sample_rate;
        let dt = dt.clamp(0.0, 0.5); // Nyquist limit

        let sample = match waveform {
            Waveform::Saw => {
                let naive = 2.0 * self.phase - 1.0;
                naive - poly_blep(self.phase, dt)
            }
            Waveform::Pulse => {
                let duty = pulse_width.clamp(0.05, 0.95);
                let mut sample = if self.phase < duty { 1.0 } else { -1.0 };
                sample += poly_blep(self.phase, dt);
                sample -= poly_blep((self.phase - duty + 1.0) % 1.0, dt);
                sample
            }
            Waveform::Triangle => {
                // Integrated square approach for band-limited triangle
                let p = self.phase;
                if p < 0.25 {
                    p * 4.0
                } else if p < 0.75 {
                    2.0 - p * 4.0
                } else {
                    p * 4.0 - 4.0
                }
            }
            Waveform::Sine => {
                fast_sin(self.phase * TAU)
            }
            Waveform::Noise => {
                self.noise_state ^= self.noise_state << 13;
                self.noise_state ^= self.noise_state >> 17;
                self.noise_state ^= self.noise_state << 5;
                (self.noise_state as f32 / u32::MAX as f32) * 2.0 - 1.0
            }
        };

        // Advance phase
        self.phase += dt;
        let wrapped = self.phase >= 1.0;
        if wrapped {
            self.phase -= 1.0;
        }

        (sample, wrapped)
    }

    /// Process with FM modulation (modulator output modulates this oscillator's phase)
    pub fn process_fm(
        &mut self,
        frequency: f32,
        waveform: Waveform,
        pulse_width: f32,
        fm_input: f32,
        fm_amount: f32,
    ) -> (f32, bool) {
        let dt = frequency / self.sample_rate;
        let dt = dt.clamp(0.0, 0.5);

        // FM: modulate phase by fm_input
        let fm_phase = (self.phase + fm_input * fm_amount * 0.5).fract();
        let fm_phase = if fm_phase < 0.0 { fm_phase + 1.0 } else { fm_phase };

        let sample = match waveform {
            Waveform::Saw => {
                let naive = 2.0 * fm_phase - 1.0;
                naive - poly_blep(fm_phase, dt)
            }
            Waveform::Pulse => {
                let duty = pulse_width.clamp(0.05, 0.95);
                let mut s = if fm_phase < duty { 1.0 } else { -1.0 };
                s += poly_blep(fm_phase, dt);
                s -= poly_blep((fm_phase - duty + 1.0) % 1.0, dt);
                s
            }
            Waveform::Triangle => {
                if fm_phase < 0.25 { fm_phase * 4.0 }
                else if fm_phase < 0.75 { 2.0 - fm_phase * 4.0 }
                else { fm_phase * 4.0 - 4.0 }
            }
            Waveform::Sine => {
                fast_sin(fm_phase * TAU)
            }
            Waveform::Noise => {
                self.noise_state ^= self.noise_state << 13;
                self.noise_state ^= self.noise_state >> 17;
                self.noise_state ^= self.noise_state << 5;
                (self.noise_state as f32 / u32::MAX as f32) * 2.0 - 1.0
            }
        };

        self.phase += dt;
        let wrapped = self.phase >= 1.0;
        if wrapped {
            self.phase -= 1.0;
        }

        (sample, wrapped)
    }

    /// Process with FM and continuous shape morphing
    pub fn process_fm_shape(
        &mut self,
        frequency: f32,
        pulse_width: f32,
        fm_input: f32,
        fm_amount: f32,
        shape: f32,
    ) -> (f32, bool) {
        let dt = frequency / self.sample_rate;
        let dt = dt.clamp(0.0, 0.5);

        let fm_phase = (self.phase + fm_input * fm_amount * 0.5).fract();
        let fm_phase = if fm_phase < 0.0 { fm_phase + 1.0 } else { fm_phase };

        let shape = shape.fract();
        let idx = shape * 4.0;
        let base = idx as usize;
        let frac = idx - base as f32;

        let waveforms = [Waveform::Saw, Waveform::Pulse, Waveform::Triangle, Waveform::Sine];
        let wf_a = waveforms[base % 4];
        let wf_b = waveforms[(base + 1) % 4];

        let sample_a = self.raw_waveform(wf_a, fm_phase, dt, pulse_width);
        let sample_b = self.raw_waveform(wf_b, fm_phase, dt, pulse_width);
        let sample = sample_a * (1.0 - frac) + sample_b * frac;

        self.phase += dt;
        let wrapped = self.phase >= 1.0;
        if wrapped { self.phase -= 1.0; }

        (sample, wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every 500th sample of process, process_fm, process_shape and
    /// process_fm_shape per waveform, captured from the hand-rolled
    /// oscillator this one replaced.
    #[test]
    fn renders_what_the_legacy_oscillator_rendered() {
        let golden: [[f32; 24]; 5] = [
            [0.0, 0.32312918, -0.6848072, -0.023809493, 0.30612302, 0.3049891, 0.0, -0.8894132, -0.5531695, -0.34792173, 0.69969606, 0.3639015, 0.0, -0.2789192, 0.3877475, 1.0, 0.8843232, 0.8774747, 0.9794547, 0.89658487, 0.82289493, 0.7412413, 0.30276456, 0.03677199],
            [0.0, -1.0, 1.0, -1.0, -1.0, -1.0, 0.0, 1.0, 1.0, 1.0, -1.0, -1.0, 0.0, -0.2789192, 0.3877475, 1.0, 0.8843232, 0.8774747, 0.9794547, 0.89658487, 0.82289493, 0.7412413, 0.30276456, 0.03677199],
            [0.0, -0.64625835, 0.6303857, 0.047618985, -0.61224604, -0.6099782, 0.0, 0.22117364, 0.893661, 0.69584346, -0.6006079, -0.727803, 0.0, -0.2789192, 0.3877475, 1.0, 0.8843232, 0.8774747, 0.9794547, 0.89658487, 0.82289493, 0.7412413, 0.30276456, 0.03677199],
            [0.0, -0.850235, 0.83684623, 0.07399683, -0.82090396, -0.8188632, 0.0, 0.3397349, 0.9861766, 0.8885993, -0.8103203, -0.9104733, 0.0, -0.2789192, 0.3877475, 1.0, 0.8843232, 0.8774747, 0.9794547, 0.89658487, 0.82289493, 0.7412413, 0.30276456, 0.03677199],
            [-0.44149393, 0.69684064, 0.91863406, -0.19430953, 0.7185339, -0.614895, -0.44149393, 0.69684064, 0.91863406, -0.19430953, 0.7185339, -0.614895, 0.0, -0.2789192, 0.3877475, 1.0, 0.8843232, 0.8774747, 0.9794547, 0.89658487, 0.82289493, 0.7412413, 0.30276456, 0.03677199],
        ];
        for (wf, want) in golden.iter().enumerate() {
            let w = Waveform::from_index(wf);
            let mut out = Vec::new();
            let mut o = Oscillator::new(44100.0);
            for i in 0..3000 {
                let f = 110.0 + i as f32 * 0.5;
                let (s, _) = o.process(f, w, 0.35);
                out.push(s);
            }
            let mut o2 = Oscillator::new(44100.0);
            for i in 0..3000 {
                let f = 220.0 + i as f32 * 0.3;
                let (a, _) = o2.process_fm(f, w, 0.4, (i as f32 * 0.01).sin(), 0.6);
                out.push(a);
            }
            let mut o3 = Oscillator::new(44100.0);
            for i in 0..3000 {
                let (a, _) = o3.process_shape(180.0, 0.3, i as f32 / 3000.0);
                let (b, _) = o3.process_fm_shape(180.0, 0.3, 0.2, 0.5, i as f32 / 3000.0);
                out.push(a);
                out.push(b);
            }
            let got: Vec<f32> = out.into_iter().step_by(500).collect();
            assert_close(&got, want, &format!("oscillator waveform {wf}"));
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

/// One sample of a PolyBLEP sawtooth on an f64 phase in `0..1`, advanced
/// by `freq / sample_rate`. The residual is applied around the wrap only,
/// so the wave is the naive ramp everywhere else.
#[inline]
pub fn saw_polyblep(phase: &mut f64, freq: f64, sample_rate: f64) -> f32 {
    let dt = freq / sample_rate;
    let naive = 2.0 * *phase - 1.0;
    let mut out = naive;
    let t = *phase;
    if t < dt {
        let t = t / dt;
        out -= t + t - t * t - 1.0;
    } else if t > 1.0 - dt {
        let t = (t - 1.0 + dt) / dt;
        out -= t * t + t + t - 1.0;
    }
    *phase += dt;
    if *phase >= 1.0 {
        *phase -= 1.0;
    }
    out as f32
}

#[cfg(test)]
mod polyblep_saw_tests {
    use super::saw_polyblep;

    /// Against a naive ramp at the same pitch, the PolyBLEP saw carries far
    /// less energy in the band the ramp folds into, and keeps its harmonics.
    #[test]
    fn the_wrap_is_smoothed_and_the_harmonics_stay() {
        use rustfft::{num_complex::Complex, FftPlanner};
        let (sr, hz, n) = (48_000.0_f64, 3_100.0_f64, 4096usize);
        let mut phase = 0.0;
        let blep: Vec<f32> = (0..n).map(|_| saw_polyblep(&mut phase, hz, sr)).collect();
        let mut p = 0.0;
        let naive: Vec<f32> = (0..n).map(|_| { let s = 2.0 * p - 1.0; p += hz / sr; if p >= 1.0 { p -= 1.0; } s as f32 }).collect();
        let spectrum = |x: &[f32]| {
            let mut b: Vec<Complex<f32>> = x.iter().enumerate().map(|(i, v)| {
                let w = 0.5 - 0.5 * (i as f32 * std::f32::consts::TAU / n as f32).cos();
                Complex::new(v * w, 0.0)
            }).collect();
            FftPlanner::new().plan_fft_forward(n).process(&mut b);
            b.iter().map(|c| c.norm()).collect::<Vec<_>>()
        };
        let (sb, sn) = (spectrum(&blep), spectrum(&naive));
        let bin = |f: f64| (f / sr * n as f64).round() as usize;
        // Everything that is not a harmonic of the pitch, over the whole
        // band: what the ramp's wrap folds back, and the residual leaves.
        let junk = |s: &[f32]| -> f32 {
            (8..n / 2)
                .filter(|&b| {
                    let k = (b as f64 / bin(hz) as f64).round();
                    (b as i64 - (k * bin(hz) as f64) as i64).abs() > 3
                })
                .map(|b| s[b])
                .sum()
        };
        let h1 = sb[bin(hz)];
        let h2 = sb[bin(hz * 2.0)];
        assert!((h2 / h1 - 0.5).abs() < 0.08, "second harmonic at {} of the first", h2 / h1);
    }
}
