//! A sample library: 32 looped sustain samples organized into
//! 8 categories of 4 variants each.
//!
//! v4 Phase 2 design note: the architecture is sample-based; the
//! samples here are procedurally rendered at engine init (additive
//! partial stacks with bowing/breath modulation + formant body for
//! strings/choir/wind, inharmonic decay for glass/metal). They share
//! the SampleAsset shape with disk-loaded WAVs, so a future
//! `samples/*.wav` directory can be loaded by the same
//! `SampleLib` API by extending `generate_all()` to read disk first
//! and fall back to procedural for missing entries.
//!
//! Procedural rendering keeps the binary small (no `include_bytes!`
//! 8 MB hit), gives every sample a clean loop point (no editor work),
//! and lets the v4 pipeline (sample-player + granular) be exercised
//! identically whether the source is procedural or recorded.

use std::f32::consts::TAU;
use std::sync::OnceLock;

/// One sample asset: a mono buffer with a documented base pitch and
/// loop region. SamplePlayer pitch-shifts via playback-rate ratio
/// against `base_freq_hz`.
pub struct SampleAsset {
    pub name:          &'static str,
    pub samples:       Vec<f32>,
    pub sample_rate:   f32,
    /// Original pitch the buffer was rendered at. The player computes
    /// playback rate as note_freq / base_freq_hz to track pitch.
    pub base_freq_hz:  f32,
    /// Loop start (samples). Skip the attack so the loop body is
    /// steady-state texture.
    pub loop_start:    usize,
    /// Loop end (samples). Player crossfades a small region around
    /// this boundary to hide the seam.
    pub loop_end:      usize,
}

/// Stable identifier of a sample within the library. The numeric
/// value is durable across rebuilds so saved presets keep pointing at
/// the same sample when the library is re-generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SampleCategory {
    StringsLow   = 0,
    StringsMid   = 1,
    ChoirFemale  = 2,
    ChoirMale    = 3,
    Glass        = 4,
    Wind         = 5,
    BowedMetal   = 6,
    SlowSynth    = 7,
}

impl SampleCategory {
    pub const ALL: &'static [SampleCategory] = &[
        SampleCategory::StringsLow, SampleCategory::StringsMid,
        SampleCategory::ChoirFemale, SampleCategory::ChoirMale,
        SampleCategory::Glass, SampleCategory::Wind,
        SampleCategory::BowedMetal, SampleCategory::SlowSynth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SampleCategory::StringsLow  => "Strings Low",
            SampleCategory::StringsMid  => "Strings Mid",
            SampleCategory::ChoirFemale => "Choir F",
            SampleCategory::ChoirMale   => "Choir M",
            SampleCategory::Glass       => "Glass",
            SampleCategory::Wind        => "Wind",
            SampleCategory::BowedMetal  => "Bowed Metal",
            SampleCategory::SlowSynth   => "Slow Synth",
        }
    }
}

pub const VARIANTS_PER_CATEGORY: usize = 4;
pub const TOTAL_SAMPLES: usize = 8 * VARIANTS_PER_CATEGORY;
pub const SAMPLE_RATE: f32 = 22_050.0;
pub const SAMPLE_LEN_SEC: f32 = 3.0;
pub const SAMPLE_LEN: usize = (SAMPLE_RATE * SAMPLE_LEN_SEC) as usize;
/// Loop body sits between LOOP_HEAD (= attack/onset) and the end
/// minus a small tail so we crossfade cleanly.
pub const LOOP_HEAD: usize    = (SAMPLE_RATE * 0.6) as usize;
pub const LOOP_FADE_LEN: usize = (SAMPLE_RATE * 0.08) as usize;

pub struct SampleLib {
    pub assets: Vec<SampleAsset>,
}

impl SampleLib {
    pub fn get(&self, idx: usize) -> Option<&SampleAsset> {
        self.assets.get(idx)
    }

    /// Convert (category, variant) to a flat index.
    pub fn flat_idx(cat: SampleCategory, variant: u8) -> usize {
        (cat as usize) * VARIANTS_PER_CATEGORY + (variant as usize % VARIANTS_PER_CATEGORY)
    }
}

static LIB: OnceLock<SampleLib> = OnceLock::new();

/// Global shared sample library — generated lazily on first access.
/// Subsequent calls return the same `&'static SampleLib`.
pub fn shared() -> &'static SampleLib {
    LIB.get_or_init(generate_all)
}

fn generate_all() -> SampleLib {
    let mut assets = Vec::with_capacity(TOTAL_SAMPLES);
    // Render 4 variants per category at musically useful base pitches
    // that span the cinematic-pad range.
    for cat in SampleCategory::ALL {
        for variant in 0..VARIANTS_PER_CATEGORY {
            assets.push(render_one(*cat, variant as u8));
        }
    }
    SampleLib { assets }
}

fn render_one(cat: SampleCategory, variant: u8) -> SampleAsset {
    let len = SAMPLE_LEN;
    // Base freq grid per variant: spread across 2 octaves so different
    // variants in the same category fit different pitch registers.
    // Categories that need very-low fundamentals (StringsLow, SlowSynth)
    // get their own base-freq scaling.
    let freq_grid = [55.0_f32, 82.41, 110.0, 164.81]; // A1, E2, A2, E3
    let base_freq = match cat {
        SampleCategory::StringsLow  => freq_grid[variant as usize] * 0.5,         // A0..E2
        SampleCategory::StringsMid  => freq_grid[variant as usize] * 2.0,         // A2..E4
        SampleCategory::ChoirFemale => freq_grid[variant as usize] * 4.0,         // A3..E5
        SampleCategory::ChoirMale   => freq_grid[variant as usize] * 1.5,         // E2..B3
        SampleCategory::Glass       => freq_grid[variant as usize] * 3.0,         // mid
        SampleCategory::Wind        => freq_grid[variant as usize] * 1.0,
        SampleCategory::BowedMetal  => freq_grid[variant as usize] * 2.5,
        SampleCategory::SlowSynth   => freq_grid[variant as usize] * 1.0,
    };
    let raw = match cat {
        SampleCategory::StringsLow | SampleCategory::StringsMid => {
            render_bowed_strings(base_freq, SAMPLE_RATE, len, variant)
        }
        SampleCategory::ChoirFemale => {
            render_choir(base_freq, SAMPLE_RATE, len, true, variant)
        }
        SampleCategory::ChoirMale => {
            render_choir(base_freq, SAMPLE_RATE, len, false, variant)
        }
        SampleCategory::Glass => {
            render_glass(base_freq, SAMPLE_RATE, len, variant)
        }
        SampleCategory::Wind => {
            render_wind(base_freq, SAMPLE_RATE, len, variant)
        }
        SampleCategory::BowedMetal => {
            render_bowed_metal(base_freq, SAMPLE_RATE, len, variant)
        }
        SampleCategory::SlowSynth => {
            render_slow_synth(base_freq, SAMPLE_RATE, len, variant)
        }
    };
    // The sample player handles loop-seam crossfade at playback time
    // (see SamplePlayer::process). Baking it here would require the
    // pre-head and tail samples to align across the loop wrap, which
    // they don't for non-loop-perfect generators.
    SampleAsset {
        name: cat.label(),
        samples: raw,
        sample_rate: SAMPLE_RATE,
        base_freq_hz: base_freq,
        loop_start: LOOP_HEAD,
        loop_end:   SAMPLE_LEN.saturating_sub(LOOP_FADE_LEN),
    }
}

// ── Generators ───────────────────────────────────────────────────────────

/// Bowed-string-like sustain. Harmonic 1..24 with rolloff matching
/// real string spectra, per-harmonic amplitude jitter that mimics
/// bowing pressure variation, plus a slow vibrato on the
/// fundamental. variant selects bow-pressure character.
fn render_bowed_strings(f0: f32, sr: f32, n: usize, variant: u8) -> Vec<f32> {
    let mut buf = vec![0.0_f32; n];
    let n_harm = 24;
    let bright = variant >= 2;
    let rolloff = if bright { 0.95 } else { 1.30 };
    let jitter_depth = 0.07 + (variant as f32) * 0.015;
    let vibrato_hz = 4.5 + (variant as f32) * 0.3;
    for h in 1..=n_harm {
        let amp = 1.0 / (h as f32).powf(rolloff);
        // Slight inharmonicity to avoid mathematical-pure stack.
        let detune = 1.0 + 0.0002 * (h as f32 - 1.0) * (h as f32 - 1.0);
        let f = f0 * h as f32 * detune;
        let phase_off = (h as f32) * 0.137;
        let jitter_rate = 3.0 + (h as f32) * 0.4;
        for i in 0..n {
            let t = i as f32 / sr;
            let vib = 1.0 + 0.003 * (TAU * vibrato_hz * t).sin();
            let jitter = 1.0 + jitter_depth * (TAU * jitter_rate * t + phase_off).sin();
            buf[i] += amp * jitter * (TAU * f * vib * t).sin();
        }
    }
    normalize(&mut buf, 0.85);
    buf
}

/// Choir-like sustain. Lower fundamental, formant peaks at the chosen
/// vowel (Ah for female, Oo for male). Adds a chorused detune layer.
fn render_choir(f0: f32, sr: f32, n: usize, female: bool, variant: u8) -> Vec<f32> {
    let mut buf = vec![0.0_f32; n];
    let n_harm = 20;
    // Formants per vowel (F1, F2) — Ah for female, Oo for male,
    // variant +/- a small amount for individuality across slots.
    let (f1, f2) = if female {
        (850.0 + (variant as f32) * 30.0, 1610.0 + (variant as f32) * 40.0)
    } else {
        (320.0 + (variant as f32) * 20.0,  920.0 + (variant as f32) * 30.0)
    };
    let q = 8.0;
    let detune_cents = 6.0 + (variant as f32) * 2.0;
    let det_ratio = 2.0_f32.powf(detune_cents / 1200.0);
    for h in 1..=n_harm {
        let f = f0 * h as f32;
        let f_det = f * det_ratio;
        let amp = formant_amp(f, f1, q) * 0.6 + formant_amp(f, f2, q) * 0.4;
        let amp = amp * (1.0 / (h as f32).powf(0.5)); // gentle rolloff
        let phase_a = (h as f32) * 0.073;
        let phase_b = (h as f32) * 0.211;
        for i in 0..n {
            let t = i as f32 / sr;
            // Slow breath modulation: 0.4 Hz tremolo
            let breath = 1.0 + 0.08 * (TAU * 0.4 * t + (h as f32) * 0.1).sin();
            buf[i] += amp * breath * 0.5 * (TAU * f * t + phase_a).sin();
            buf[i] += amp * breath * 0.5 * (TAU * f_det * t + phase_b).sin();
        }
    }
    normalize(&mut buf, 0.80);
    buf
}

fn formant_amp(freq: f32, center: f32, q: f32) -> f32 {
    let bw = center / q;
    let diff = freq - center;
    1.0 / (1.0 + (diff / bw).powi(2))
}

/// Glass / singing-bowl: inharmonic partial stack with a long
/// exponential decay (we render the steady-state portion so the
/// looper holds it). Variant chooses partial spacing.
fn render_glass(f0: f32, sr: f32, n: usize, variant: u8) -> Vec<f32> {
    let mut buf = vec![0.0_f32; n];
    // Inharmonic ratios from struck-bowl spectra (no harmonic series).
    let ratios = match variant {
        0 => [1.0_f32, 2.76, 5.40, 8.93, 13.34, 18.64],
        1 => [1.0, 2.40, 4.94, 8.20, 12.10, 16.50],
        2 => [1.0, 3.10, 5.80, 9.30, 13.90, 19.10],
        _ => [1.0, 2.65, 5.20, 8.70, 12.80, 17.80],
    };
    for (i, &r) in ratios.iter().enumerate() {
        let f = f0 * r;
        let amp = 1.0 / (1.0 + (i as f32) * 0.6);
        let phase = (i as f32) * 0.291;
        // Each partial has its own slow beating with a tiny detuned twin.
        let det = 1.0 + 0.002 * (i as f32 + 1.0);
        for k in 0..n {
            let t = k as f32 / sr;
            let a = amp * (-t * 0.15 * (i as f32 + 1.0)).exp().max(0.3);
            buf[k] += a * 0.6 * (TAU * f * t + phase).sin();
            buf[k] += a * 0.4 * (TAU * f * det * t + phase).sin();
        }
    }
    normalize(&mut buf, 0.85);
    buf
}

/// Wind / breath texture: pink noise through a steep LP + a body
/// resonance at f0. Looped for steady texture, variant changes
/// brightness and resonance Q.
fn render_wind(f0: f32, sr: f32, n: usize, variant: u8) -> Vec<f32> {
    let mut buf = vec![0.0_f32; n];
    // Deterministic LCG for repeatable rendering.
    let mut rng: u64 = 0x9E3779B97F4A7C15 ^ ((f0 as u64) << 8) ^ (variant as u64);
    let mut pink_state = [0.0_f32; 5];
    let cutoff_norm = 0.35 + (variant as f32) * 0.05;
    let body_q = 6.0 + (variant as f32) * 1.5;
    // 2-pole resonant peak at f0 + 1-pole LP
    let bw = f0 / body_q;
    let r  = 1.0 - (TAU * bw / sr).min(0.99);
    let theta = TAU * f0 / sr;
    let a1 = -2.0 * r * theta.cos();
    let a2 =  r * r;
    let mut z1 = 0.0_f32;
    let mut z2 = 0.0_f32;
    // 1-pole LP
    let g = 1.0 - (-TAU * 20.0 * 2.0_f32.powf(cutoff_norm * 5.0) / sr).exp();
    let mut lp = 0.0_f32;
    for i in 0..n {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let white = ((rng >> 33) as f32 / (1u64 << 31) as f32) - 1.0;
        // Voss-McCartney pink noise approximation.
        pink_state[0] = 0.99886 * pink_state[0] + white * 0.0555179;
        pink_state[1] = 0.99332 * pink_state[1] + white * 0.0750759;
        pink_state[2] = 0.96900 * pink_state[2] + white * 0.1538520;
        pink_state[3] = 0.86650 * pink_state[3] + white * 0.3104856;
        pink_state[4] = 0.55000 * pink_state[4] + white * 0.5329522;
        let pink = pink_state.iter().sum::<f32>() * 0.11;
        // 1-pole LP
        lp += g * (pink - lp);
        // 2-pole body resonance
        let y = lp - a1 * z1 - a2 * z2;
        z2 = z1; z1 = y;
        buf[i] = y;
    }
    normalize(&mut buf, 0.7);
    buf
}

/// Bowed-metal / waterphone: sparse high-Q inharmonic peaks with
/// slow bowing-driven amplitude modulation.
fn render_bowed_metal(f0: f32, sr: f32, n: usize, variant: u8) -> Vec<f32> {
    let mut buf = vec![0.0_f32; n];
    let ratios = match variant {
        0 => [1.0_f32, 1.59, 2.14, 2.91, 4.12, 5.83],
        1 => [1.0, 1.71, 2.45, 3.28, 4.55, 6.30],
        2 => [1.0, 1.83, 2.62, 3.71, 5.07, 7.15],
        _ => [1.0, 2.00, 2.83, 3.97, 5.50, 7.78],
    };
    for (i, &r) in ratios.iter().enumerate() {
        let f = f0 * r;
        let amp = 1.0 / (1.0 + (i as f32) * 0.4);
        let bow_rate = 0.3 + (i as f32) * 0.05; // slow bowing
        let phase = (i as f32) * 0.617;
        for k in 0..n {
            let t = k as f32 / sr;
            let bow = (0.7 + 0.3 * (TAU * bow_rate * t).sin()).max(0.0);
            buf[k] += amp * bow * (TAU * f * t + phase).sin();
        }
    }
    // Slight tanh for character.
    for s in buf.iter_mut() { *s = (*s * 1.5).tanh(); }
    normalize(&mut buf, 0.75);
    buf
}

/// Slow synth pad: detuned super-saw stack with slow LFO filter sweep.
fn render_slow_synth(f0: f32, sr: f32, n: usize, variant: u8) -> Vec<f32> {
    let mut buf = vec![0.0_f32; n];
    let voices = 5_usize;
    let detune_cents = 8.0 + (variant as f32) * 2.0;
    let det_table = [-1.0_f32, -0.5, 0.0, 0.5, 1.0];
    let cutoff_lfo_hz = 0.15 + (variant as f32) * 0.05;
    for (vi, &d) in det_table.iter().enumerate().take(voices) {
        let f = f0 * 2.0_f32.powf((d * detune_cents) / 1200.0);
        let phase_off = (vi as f32) * 0.231;
        for i in 0..n {
            let t = i as f32 / sr;
            // Bandlimited-ish saw via summed sines (first 30 harmonics).
            let mut s = 0.0_f32;
            for h in 1..=30 {
                let amp = 1.0 / h as f32;
                s += amp * (TAU * (f * h as f32) * t + phase_off).sin();
            }
            // Filter envelope
            let cutoff = 0.4 + 0.2 * (TAU * cutoff_lfo_hz * t).sin();
            let g = 1.0 - (-TAU * 200.0 * 50.0_f32.powf(cutoff) / sr).exp();
            buf[i] += s * g * 0.2;
        }
    }
    // Single-pole smoothing
    let alpha = 0.05;
    let mut z = 0.0_f32;
    for s in buf.iter_mut() {
        z = z + alpha * (*s - z);
        *s = z;
    }
    normalize(&mut buf, 0.8);
    buf
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn normalize(buf: &mut [f32], target_peak: f32) {
    let peak = buf.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
    if peak < 1e-6 { return; }
    let g = target_peak / peak;
    for s in buf.iter_mut() { *s *= g; }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_has_32_samples() {
        let lib = shared();
        assert_eq!(lib.assets.len(), TOTAL_SAMPLES);
        assert_eq!(TOTAL_SAMPLES, 32);
    }

    #[test]
    fn every_sample_has_audible_content() {
        let lib = shared();
        for (i, a) in lib.assets.iter().enumerate() {
            let peak = a.samples.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
            assert!(peak > 0.1, "sample {i} ({}) has near-zero peak {peak}", a.name);
            assert!(a.loop_end > a.loop_start, "sample {i}: loop_end <= loop_start");
            assert!(a.base_freq_hz > 10.0 && a.base_freq_hz < 5000.0,
                "sample {i}: base_freq_hz {} out of range", a.base_freq_hz);
        }
    }

    #[test]
    fn flat_idx_matches_category_layout() {
        for (ci, cat) in SampleCategory::ALL.iter().enumerate() {
            for v in 0..VARIANTS_PER_CATEGORY as u8 {
                assert_eq!(
                    SampleLib::flat_idx(*cat, v),
                    ci * VARIANTS_PER_CATEGORY + v as usize
                );
            }
        }
    }
}
