//! Spectral resynthesis: STFT-analyze a real sample (a voice, a pad, anything)
//! into a time-varying spectrum, then play it back additively so it can be
//! pitched by a note and scrubbed, blurred, shifted or frozen.
//!
//! Design: rather than track individual partials across frames (fragile), we
//! pick a FIXED set of anchor frequencies from the average spectrum, then store
//! each anchor's AMPLITUDE per analysis frame. On playback each anchor is a
//! stable oscillator (constant frequency, so no clicks) whose amplitude
//! interpolates across frames as `position` scans - the spectral ENVELOPE
//! evolves the way the source did, which is what makes an analysed pad or choir
//! sound alive. Analysis is offline (allocates, not RT); playback is per-sample.

use std::sync::Arc;
use std::f32::consts::TAU;

use rustfft::{FftPlanner, num_complex::Complex};

use crate::fastmath::fast_sin;

/// Number of anchor partials retained from the analysis.
pub const NP: usize = 48;

const FFT_SIZE: usize = 2048;
const HOP: usize = 512;

/// A sample analysed into fixed anchor frequencies + per-frame amplitudes.
#[derive(Clone)]
pub struct SpectralModel {
    /// Anchor partial frequencies in Hz (0 = unused slot).
    anchor_freqs: [f32; NP],
    /// Per analysis frame, the amplitude of each anchor. Outer len = frame count.
    frame_amps: Vec<[f32; NP]>,
    /// The analysed fundamental, so a played note transposes the whole spectrum.
    pub base_hz: f32,
}

impl SpectralModel {
    /// STFT-analyse a mono sample. Never call on the audio thread.
    pub fn analyze(samples: &[f32], sample_rate: f32) -> SpectralModel {
        let bin_hz = sample_rate / FFT_SIZE as f32;
        // Hann window.
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (TAU * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let half = FFT_SIZE / 2;

        // Per-frame magnitude half-spectra.
        let mut mags: Vec<Vec<f32>> = Vec::new();
        let mut pos = 0;
        while pos + FFT_SIZE <= samples.len() {
            let mut buf: Vec<Complex<f32>> = (0..FFT_SIZE)
                .map(|i| Complex::new(samples[pos + i] * window[i], 0.0))
                .collect();
            fft.process(&mut buf);
            mags.push((0..half).map(|k| buf[k].norm()).collect());
            pos += HOP;
        }
        if mags.is_empty() {
            // Too short to analyse: a single silent frame at a safe pitch.
            return SpectralModel {
                anchor_freqs: [0.0; NP],
                frame_amps: vec![[0.0; NP]],
                base_hz: 220.0,
            };
        }

        // Average spectrum -> pick the anchor frequencies (loudest local maxima).
        let n_frames = mags.len() as f32;
        let mut avg = vec![0.0f32; half];
        for m in &mags {
            for k in 0..half { avg[k] += m[k] / n_frames; }
        }
        let max_avg = avg.iter().cloned().fold(0.0f32, f32::max).max(1e-9);
        let thresh = max_avg * 1e-3;
        let mut peaks: Vec<(f32, f32)> = Vec::new(); // (freq, avg-mag)
        for k in 1..half - 1 {
            if avg[k] > avg[k - 1] && avg[k] >= avg[k + 1] && avg[k] > thresh {
                // Parabolic interpolation for a sub-bin frequency estimate.
                let (a, b, c) = (avg[k - 1], avg[k], avg[k + 1]);
                let denom = a - 2.0 * b + c;
                let delta = if denom.abs() > 1e-12 { 0.5 * (a - c) / denom } else { 0.0 };
                peaks.push(((k as f32 + delta) * bin_hz, b));
            }
        }
        peaks.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
        peaks.truncate(NP);

        let mut anchor_freqs = [0.0f32; NP];
        for (i, p) in peaks.iter().enumerate() { anchor_freqs[i] = p.0; }

        // Per frame, sample each anchor's amplitude out of that frame's spectrum
        // (linear interpolation between the two nearest bins).
        let sample_mag = |m: &[f32], freq: f32| -> f32 {
            let b = freq / bin_hz;
            let i0 = b.floor() as usize;
            if i0 + 1 >= half { return 0.0; }
            let frac = b - i0 as f32;
            m[i0] * (1.0 - frac) + m[i0 + 1] * frac
        };
        let mut frame_amps: Vec<[f32; NP]> = Vec::with_capacity(mags.len());
        for m in &mags {
            let mut fa = [0.0f32; NP];
            for k in 0..NP {
                if anchor_freqs[k] > 0.0 { fa[k] = sample_mag(m, anchor_freqs[k]); }
            }
            frame_amps.push(fa);
        }

        // Fundamental estimate: the lowest anchor whose average amp is within
        // 25% of the strongest (so a weak sub-harmonic doesn't hijack it).
        let strongest = peaks.first().map(|p| p.1).unwrap_or(1.0).max(1e-9);
        let base_hz = peaks.iter()
            .filter(|p| p.1 >= strongest * 0.25)
            .map(|p| p.0)
            .fold(f32::INFINITY, f32::min);
        let base_hz = if base_hz.is_finite() && base_hz > 0.0 { base_hz } else { 220.0 };

        SpectralModel { anchor_freqs, frame_amps, base_hz }
    }

    pub fn frame_count(&self) -> usize { self.frame_amps.len() }

    /// A silent placeholder, for a voice that has no model loaded yet.
    pub fn empty() -> SpectralModel {
        SpectralModel { anchor_freqs: [0.0; NP], frame_amps: vec![[0.0; NP]], base_hz: 220.0 }
    }
}

static MODELS: std::sync::OnceLock<Vec<Arc<SpectralModel>>> = std::sync::OnceLock::new();

/// Analyzed models for the whole procedural sample bank, built once (STFT over
/// every asset). The first call is heavy, so it must run OFF the audio thread;
/// prefer `warm_in_background`.
pub fn shared_models() -> &'static Vec<Arc<SpectralModel>> {
    MODELS.get_or_init(|| {
        let lib = crate::samples::shared();
        (0..crate::samples::TOTAL_SAMPLES)
            .map(|i| {
                let s: &[f32] = lib.get(i).map(|a| a.samples.as_slice()).unwrap_or(&[]);
                Arc::new(SpectralModel::analyze(s, 48_000.0))
            })
            .collect()
    })
}

/// Kick off the model build on a background thread (idempotent). Call from an
/// engine constructor so the models are ready by the time a Resynth voice runs,
/// without blocking track construction.
pub fn warm_in_background() {
    if MODELS.get().is_none() {
        std::thread::spawn(|| { let _ = shared_models(); });
    }
}

/// The model for a sample id, NON-BLOCKING: `Some` once the bank is warmed,
/// `None` while it is still building. Audio-thread safe (never blocks, never
/// analyzes).
pub fn try_model(idx: usize) -> Option<Arc<SpectralModel>> {
    let m = MODELS.get()?;
    let i = idx.min(m.len().saturating_sub(1));
    m.get(i).cloned()
}

/// A per-voice player over a `SpectralModel`.
#[derive(Clone)]
pub struct SpectralResynth {
    model: Arc<SpectralModel>,
    phases: [f32; NP],
    amps: [f32; NP],
    sample_rate: f32,
}

impl SpectralResynth {
    pub fn new(model: Arc<SpectralModel>, sample_rate: f32) -> Self {
        Self {
            model,
            // Golden-ratio phase spread so partials don't start coherent.
            phases: std::array::from_fn(|k| (k as f32 * 0.6180339) % 1.0),
            amps: [0.0; NP],
            sample_rate: sample_rate.max(1.0),
        }
    }

    /// The analyzed fundamental, so a caller can derive note_ratio = note / base.
    pub fn base_hz(&self) -> f32 { self.model.base_hz }

    /// Swap the model in place (keeps phases/amps, so a sample change glides).
    pub fn set_model(&mut self, model: Arc<SpectralModel>) { self.model = model; }

    /// One stereo sample.
    /// * `note_ratio`  played frequency / model base pitch (1.0 = as analysed).
    /// * `position`    0..1 scrub through the analysis frames (hold it = freeze).
    /// * `shift`       multiplies every partial frequency (1.0 = none).
    /// * `blur`        0..1 amplitude smoothing (smears transients into a wash).
    #[inline]
    pub fn process(&mut self, note_ratio: f32, position: f32, shift: f32, blur: f32) -> (f32, f32) {
        let m = &self.model;
        let nf = m.frame_amps.len();
        if nf == 0 { return (0.0, 0.0); }
        let fp = position.clamp(0.0, 1.0) * (nf - 1) as f32;
        let i0 = fp as usize;
        let frac = fp - i0 as f32;
        let i1 = (i0 + 1).min(nf - 1);
        let a0 = &m.frame_amps[i0];
        let a1 = &m.frame_amps[i1];

        let dt = 1.0 / self.sample_rate;
        let nyq = self.sample_rate * 0.45;
        // Blur -> a per-sample slew of the amplitude toward its target. blur 0 =
        // follow exactly; blur -> 1 = long smear.
        let slew = (1.0 - blur.clamp(0.0, 1.0) * 0.9995).max(5.0e-4);

        let (mut l, mut r) = (0.0f32, 0.0f32);
        let mut norm = 0.0f32;
        for k in 0..NP {
            let fa = m.anchor_freqs[k];
            if fa <= 0.0 { continue; }
            let target = a0[k] * (1.0 - frac) + a1[k] * frac;
            self.amps[k] += (target - self.amps[k]) * slew;
            let amp = self.amps[k];
            if amp <= 1.0e-6 { continue; }
            let freq = fa * note_ratio * shift;
            if freq >= nyq || freq <= 0.0 { continue; }
            let ph = &mut self.phases[k];
            *ph += freq * dt;
            if *ph >= 1.0 { *ph -= ph.floor(); }
            let s = fast_sin(*ph * TAU) * amp;
            if k % 2 == 0 { l += s; r += s * 0.7; } else { l += s * 0.7; r += s; }
            norm += amp;
        }
        let g = 0.7 / norm.max(1.0e-3);
        (l * g, r * g)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, secs: f32, sr: f32) -> Vec<f32> {
        (0..(secs * sr) as usize)
            .map(|i| (TAU * freq * i as f32 / sr).sin() * 0.7)
            .collect()
    }

    #[test]
    fn analysis_finds_the_tone_frequency() {
        let sr = 48_000.0;
        let m = SpectralModel::analyze(&sine(440.0, 1.0, sr), sr);
        // The base pitch estimate should land near 440 Hz (within a few %).
        let cents = 1200.0 * (m.base_hz / 440.0).log2();
        assert!(cents.abs() < 60.0, "base_hz {} not near 440 ({cents:+.0} cents)", m.base_hz);
    }

    #[test]
    fn resynth_reproduces_an_audible_tone_at_pitch() {
        let sr = 48_000.0;
        let model = Arc::new(SpectralModel::analyze(&sine(220.0, 1.0, sr), sr));
        let mut voice = SpectralResynth::new(model.clone(), sr);
        // Play as analysed (note_ratio 1), scrubbing the middle of the sample.
        let mut peak = 0.0f32;
        for _ in 0..24_000 {
            let (l, r) = voice.process(1.0, 0.5, 1.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak > 1e-2, "resynth silent (peak {peak})");
        assert!(peak < 2.0, "resynth too hot (peak {peak})");
    }

    #[test]
    fn shift_raises_the_partials() {
        // A shift of 2.0 should roughly double the dominant frequency: measure a
        // zero-crossing rate proxy on the mono output.
        let sr = 48_000.0;
        let model = Arc::new(SpectralModel::analyze(&sine(200.0, 1.0, sr), sr));
        let zcr = |shift: f32| {
            let mut v = SpectralResynth::new(model.clone(), sr);
            let mut prev = 0.0f32;
            let mut cross = 0usize;
            for _ in 0..24_000 {
                let (l, _) = v.process(1.0, 0.5, shift, 0.0);
                if (prev <= 0.0 && l > 0.0) { cross += 1; }
                prev = l;
            }
            cross
        };
        let base = zcr(1.0);
        let up = zcr(2.0);
        assert!(up as f32 > base as f32 * 1.5,
            "shift did not raise pitch: {base} -> {up} crossings");
    }

    #[test]
    fn two_tones_are_both_captured() {
        let sr = 48_000.0;
        let mut s = sine(300.0, 1.0, sr);
        for (i, x) in s.iter_mut().enumerate() {
            *x += (TAU * 900.0 * i as f32 / sr).sin() * 0.5;
        }
        let m = SpectralModel::analyze(&s, sr);
        let has = |f: f32| m.anchor_freqs.iter().any(|&a| a > 0.0 && (a - f).abs() < f * 0.05);
        assert!(has(300.0), "300 Hz partial missing");
        assert!(has(900.0), "900 Hz partial missing");
    }

    #[test]
    fn a_short_sample_is_safe() {
        let m = SpectralModel::analyze(&[0.1, -0.1, 0.2], 48_000.0);
        assert_eq!(m.frame_count(), 1);
        let mut v = SpectralResynth::new(Arc::new(m), 48_000.0);
        let (l, _) = v.process(1.0, 0.0, 1.0, 0.0);
        assert!(l.is_finite());
    }
}
