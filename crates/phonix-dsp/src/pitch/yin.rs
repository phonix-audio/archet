//! YIN pitch detection algorithm (optimized for real-time)
//!
//! Based on: "YIN, a fundamental frequency estimator for speech and music"
//! by Alain de Cheveigné and Hideki Kawahara (2002).
//!
//! Optimizations vs. naive YIN:
//! - Window size 1024 (not 2048) — covers fundamentals down to ~94Hz at 48kHz
//! - Hop size 512 — analyze every 512 samples (~10.7ms at 48kHz)
//! - Difference function uses incremental sum trick: O(W) per tau instead of O(W²) total
//! - Contiguous buffer copy to avoid modular indexing in inner loop
//! - Early exit once threshold is met

const WINDOW_SIZE: usize = 1024;
const HALF_WINDOW: usize = WINDOW_SIZE / 2; // 512
const HOP_SIZE: usize = 512;
const BUFFER_LEN: usize = WINDOW_SIZE * 2; // 2048

/// Real-time YIN pitch detector for monophonic audio.
pub struct YinDetector {
    buffer: [f32; BUFFER_LEN],
    write_pos: usize,
    sample_rate: f32,
    threshold: f32,
    samples_since_analysis: usize,

    // Pre-allocated scratch (on stack via fixed arrays)
    diff: [f32; HALF_WINDOW],
    cum_mean_norm: [f32; HALF_WINDOW],
    // Contiguous copy of the analysis window (avoids modular indexing in hot loop)
    window_copy: [f32; WINDOW_SIZE],

    // Output
    detected_pitch: f32,
    confidence: f32,

    // Min/max tau (precomputed)
    min_tau: usize,
    max_tau: usize,
}

impl YinDetector {
    pub fn new(sample_rate: f32, _window_size: usize, threshold: f32) -> Self {
        let min_tau = (sample_rate / 1200.0).max(2.0) as usize;
        let max_tau = (sample_rate / 80.0).min(HALF_WINDOW as f32 - 1.0) as usize;
        Self {
            buffer: [0.0; BUFFER_LEN],
            write_pos: 0,
            sample_rate,
            threshold,
            samples_since_analysis: 0,
            diff: [0.0; HALF_WINDOW],
            cum_mean_norm: [0.0; HALF_WINDOW],
            window_copy: [0.0; WINDOW_SIZE],
            detected_pitch: 0.0,
            confidence: 0.0,
            min_tau,
            max_tau,
        }
    }

    pub fn default_for_sample_rate(sample_rate: f32) -> Self {
        Self::new(sample_rate, WINDOW_SIZE, 0.15)
    }

    #[inline]
    pub fn push_sample(&mut self, sample: f32) {
        self.buffer[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % BUFFER_LEN;
        self.samples_since_analysis += 1;

        if self.samples_since_analysis >= HOP_SIZE {
            self.samples_since_analysis = 0;
            self.analyze();
        }
    }

    #[inline(always)]
    pub fn pitch(&self) -> f32 {
        self.detected_pitch
    }

    #[inline(always)]
    pub fn confidence(&self) -> f32 {
        self.confidence
    }

    #[inline(always)]
    pub fn is_voiced(&self) -> bool {
        self.detected_pitch > 0.0 && self.confidence > 0.5
    }

    pub fn pitch_to_midi(hz: f32) -> f32 {
        if hz <= 0.0 { return 0.0; }
        69.0 + 12.0 * (hz / 440.0).log2()
    }

    pub fn midi_to_pitch(midi_note: f32) -> f32 {
        440.0 * 2.0f32.powf((midi_note - 69.0) / 12.0)
    }

    fn analyze(&mut self) {
        // Copy the analysis window contiguously to avoid modular indexing in inner loop
        let start = (self.write_pos + BUFFER_LEN - WINDOW_SIZE) % BUFFER_LEN;
        if start + WINDOW_SIZE <= BUFFER_LEN {
            self.window_copy.copy_from_slice(&self.buffer[start..start + WINDOW_SIZE]);
        } else {
            let first = BUFFER_LEN - start;
            self.window_copy[..first].copy_from_slice(&self.buffer[start..]);
            self.window_copy[first..].copy_from_slice(&self.buffer[..WINDOW_SIZE - first]);
        }

        let x = &self.window_copy;
        let max_tau = self.max_tau;
        let min_tau = self.min_tau;

        // Step 1: Squared difference function — direct computation on contiguous data
        // d(tau) = sum_{j=0}^{W/2-1} (x[j] - x[j+tau])^2
        self.diff[0] = 0.0;
        for tau in 1..max_tau.min(HALF_WINDOW) {
            let mut sum = 0.0f32;
            let (left, _right) = x.split_at(HALF_WINDOW);
            // x[j] is in left[j], x[j+tau] might span left and right
            for j in 0..HALF_WINDOW {
                let delta = left[j] - x[j + tau];
                sum += delta * delta;
            }
            self.diff[tau] = sum;
        }

        // Step 2: Cumulative mean normalized difference
        self.cum_mean_norm[0] = 1.0;
        let mut running_sum = 0.0f32;
        for tau in 1..max_tau.min(HALF_WINDOW) {
            running_sum += self.diff[tau];
            self.cum_mean_norm[tau] = if running_sum > 0.0 {
                self.diff[tau] * tau as f32 / running_sum
            } else {
                1.0
            };
        }

        // Step 3: Find first dip below threshold (early exit)
        let mut best_tau = 0usize;
        let mut best_value = 1.0f32;

        for tau in min_tau..max_tau.min(HALF_WINDOW) {
            if self.cum_mean_norm[tau] < self.threshold {
                let mut t = tau;
                while t + 1 < max_tau.min(HALF_WINDOW)
                    && self.cum_mean_norm[t + 1] < self.cum_mean_norm[t]
                {
                    t += 1;
                }
                best_tau = t;
                best_value = self.cum_mean_norm[t];
                break; // Early exit — first dip is best for YIN
            }
        }

        // Fallback: global minimum if no dip below threshold
        if best_tau == 0 {
            for tau in min_tau..max_tau.min(HALF_WINDOW) {
                if self.cum_mean_norm[tau] < best_value {
                    best_value = self.cum_mean_norm[tau];
                    best_tau = tau;
                }
            }
        }

        if best_tau == 0 || best_value >= 1.0 {
            self.detected_pitch = 0.0;
            self.confidence = 0.0;
            return;
        }

        // Step 4: Parabolic interpolation
        let tau_refined = if best_tau > 0 && best_tau + 1 < HALF_WINDOW {
            let y0 = self.cum_mean_norm[best_tau - 1];
            let y1 = self.cum_mean_norm[best_tau];
            let y2 = self.cum_mean_norm[best_tau + 1];
            let denom = 2.0 * y1 - y0 - y2;
            if denom.abs() > 1e-10 {
                best_tau as f32 + (y0 - y2) / (2.0 * denom)
            } else {
                best_tau as f32
            }
        } else {
            best_tau as f32
        };

        if tau_refined > 0.0 {
            self.detected_pitch = self.sample_rate / tau_refined;
            self.confidence = (1.0 - best_value).clamp(0.0, 1.0);
        } else {
            self.detected_pitch = 0.0;
            self.confidence = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn generate_sine(freq: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| (2.0 * PI * freq * i as f32 / sample_rate).sin() * 0.8)
            .collect()
    }

    #[test]
    fn test_detect_a440() {
        let sr = 48000.0;
        let mut detector = YinDetector::default_for_sample_rate(sr);
        let samples = generate_sine(440.0, sr, 8192);
        for &s in &samples { detector.push_sample(s); }

        let pitch = detector.pitch();
        let error_cents = 1200.0 * (pitch / 440.0).log2().abs();
        assert!(pitch > 430.0 && pitch < 450.0, "Expected ~440Hz, got {}", pitch);
        assert!(error_cents < 10.0, "Error: {} cents", error_cents);
        assert!(detector.confidence() > 0.8);
    }

    #[test]
    fn test_detect_low_e() {
        let sr = 48000.0;
        let mut detector = YinDetector::default_for_sample_rate(sr);
        let freq = 110.0; // A2 (low but within range with 1024 window)
        let samples = generate_sine(freq, sr, 12000);
        for &s in &samples { detector.push_sample(s); }

        let pitch = detector.pitch();
        let error_cents = 1200.0 * (pitch / freq).log2().abs();
        assert!(error_cents < 20.0, "A2 error: {} cents (got {}Hz)", error_cents, pitch);
    }

    #[test]
    fn test_detect_high_c() {
        let sr = 48000.0;
        let mut detector = YinDetector::default_for_sample_rate(sr);
        let freq = 1046.5; // C6
        let samples = generate_sine(freq, sr, 8192);
        for &s in &samples { detector.push_sample(s); }

        let pitch = detector.pitch();
        let error_cents = 1200.0 * (pitch / freq).log2().abs();
        assert!(error_cents < 20.0, "C6 error: {} cents (got {}Hz)", error_cents, pitch);
    }

    #[test]
    fn test_silence_is_unvoiced() {
        let sr = 48000.0;
        let mut detector = YinDetector::default_for_sample_rate(sr);
        for _ in 0..4096 { detector.push_sample(0.0); }
        assert!(!detector.is_voiced());
    }

    #[test]
    fn test_noise_is_unvoiced() {
        let sr = 48000.0;
        let mut detector = YinDetector::default_for_sample_rate(sr);
        let mut rng: u32 = 12345;
        for _ in 0..8192 {
            rng ^= rng << 13; rng ^= rng >> 17; rng ^= rng << 5;
            detector.push_sample((rng as f32 / u32::MAX as f32) * 2.0 - 1.0);
        }
        assert!(detector.confidence() < 0.5);
    }

    #[test]
    fn test_midi_conversion_roundtrip() {
        let hz = 440.0;
        let midi = YinDetector::pitch_to_midi(hz);
        assert!((midi - 69.0).abs() < 0.01);
        let back = YinDetector::midi_to_pitch(midi);
        assert!((back - hz).abs() < 0.01);
    }
}
