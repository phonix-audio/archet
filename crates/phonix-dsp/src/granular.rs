//! Granular synthesis over a sample asset.
//!
//! 16 simultaneous grain voices per layer instance, scheduled at
//! configurable density. Each grain reads from the global sample
//! library at a position (with scatter) and pitch (with scatter),
//! windowed by a Hann envelope.
//!
//! Hann windows by Chebyshev recurrence; the source is a `SampleAsset`,
//! and the engine supports position freeze and reverse playback for
//! frozen / reverse textures.

use std::f32::consts::TAU;

use super::samples::SampleAsset;

pub const MAX_GRAINS: usize = 16;

#[derive(Default, Copy, Clone)]
struct Grain {
    /// True when this grain is currently sounding.
    active:    bool,
    /// Fractional source position (samples).
    pos:       f64,
    /// Playback rate (source samples per engine sample). Negative for
    /// reverse playback.
    rate:      f64,
    /// Samples remaining in this grain's lifetime.
    remaining: usize,
    /// Constant-power L/R gain.
    gain_l:    f32,
    gain_r:    f32,
    /// Hann window state via Chebyshev recurrence.
    /// w(n) = 0.5 - 0.5*cos(2*pi*n/N-1)
    /// Tracked as cos(phase) with incremental update.
    cos_phase: f32,
    cos_step:  f32,    // 2 * cos(2*pi / N)
    cos_prev:  f32,    // cos((n-1)*step)
    cos_curr:  f32,    // cos(n*step)
}

pub struct Granular {
    sr: f32,
    /// Grain pool. Round-robin spawn into the next idle slot.
    grains: [Grain; MAX_GRAINS],
    /// Density accumulator. Increments by `density_g_per_sec / sr`
    /// each sample; when >= 1.0 a grain spawns and it wraps.
    spawn_acc: f32,
    /// Scrubbing position (the "playhead" the grains spawn near
    /// when not frozen). Advances at note's pitch rate.
    scrub_pos: f64,
    scrub_rate: f64,
    cached_freq: f32,
    /// Per-spawn deterministic LCG seed.
    rng:       u64,
    /// True while the voice is active (note held).
    sounding:  bool,
}

impl Default for Granular {
    fn default() -> Self { Self::new(48_000.0) }
}

impl Clone for Granular {
    fn clone(&self) -> Self {
        Self {
            sr: self.sr,
            grains: [Grain::default(); MAX_GRAINS],
            spawn_acc: 0.0,
            scrub_pos: 0.0,
            scrub_rate: 1.0,
            cached_freq: 0.0,
            rng: self.rng,
            sounding: false,
        }
    }
}

impl Granular {
    pub fn new(sr: f32) -> Self {
        Self {
            sr,
            grains: [Grain::default(); MAX_GRAINS],
            spawn_acc: 0.0,
            scrub_pos: 0.0,
            scrub_rate: 1.0,
            cached_freq: 0.0,
            rng: 0xDEAD_BEEF_CAFE_F00D,
            sounding: false,
        }
    }

    pub fn reset_state(&mut self) {
        for g in self.grains.iter_mut() { g.active = false; }
        self.spawn_acc = 0.0;
        self.scrub_pos = 0.0;
        self.cached_freq = 0.0;
        self.sounding = false;
    }

    /// Trigger granular cloud against a specific buffer. The caller
    /// owns the `SampleAsset` and is responsible for resolving the
    /// source (a library lookup, a tape buffer wrap). The engine itself has
    /// no global state.
    pub fn note_on(&mut self, asset: &SampleAsset, freq_hz: f32) {
        // Start scrubbing at the loop_start (skip the attack onset
        // which often has render-time transients).
        self.scrub_pos = asset.loop_start as f64;
        self.update_scrub_rate(asset, freq_hz);
        // Don't wipe in-flight grains on retrigger — let the previous
        // note's tail decay naturally with the new note's grains
        // layered on top (a more organic crossover).
        self.sounding = true;
    }

    pub fn note_off(&mut self) {
        // Stop spawning new grains. In-flight grains complete their
        // Hann window naturally, so the release tail is built-in.
        self.sounding = false;
    }

    fn update_scrub_rate(&mut self, asset: &SampleAsset, freq_hz: f32) {
        if freq_hz < 1.0 || asset.base_freq_hz < 1.0 {
            self.scrub_rate = 1.0;
        } else {
            self.scrub_rate = (freq_hz as f64 / asset.base_freq_hz as f64)
                            * (asset.sample_rate as f64 / self.sr as f64);
        }
        self.cached_freq = freq_hz;
    }

    /// Render one stereo sample.
    /// Params:
    /// - `freq_hz`: pitch tracking (drives scrub rate when not frozen)
    /// - `density_g_per_sec`: 0.5..50 grains/sec
    /// - `size_ms`: 10..500 grain duration
    /// - `pitch_jit`: 0..1 = ±semitones random
    /// - `pos_jit_ms`: 0..200 random position scatter
    /// - `pan_jit`: 0..1 random L/R spread per grain
    /// - `freeze`: 0..1 mix between live scrub (0) and frozen at
    ///   the captured scrub_pos (1)
    /// - `reverse`: 0..1 probability that a new grain plays in reverse
    pub fn process(
        &mut self,
        asset:             &SampleAsset,
        freq_hz:           f32,
        density_g_per_sec: f32,
        size_ms:           f32,
        pitch_jit:         f32,
        pos_jit_ms:        f32,
        pan_jit:           f32,
        freeze:            f32,
        reverse:           f32,
    ) -> (f32, f32) {
        if (freq_hz - self.cached_freq).abs() > 1e-3 {
            self.update_scrub_rate(asset, freq_hz);
        }

        // Advance scrub position when not frozen. `freeze` mixes the
        // applied rate: freeze=1 -> stationary playhead.
        let freeze = freeze.clamp(0.0, 1.0);
        let eff_rate = self.scrub_rate * (1.0 - freeze as f64);
        self.scrub_pos += eff_rate;
        // Keep scrub_pos inside the loop region.
        let loop_lo = asset.loop_start as f64;
        let loop_hi = asset.loop_end as f64;
        let span = (loop_hi - loop_lo).max(1.0);
        if self.scrub_pos < loop_lo { self.scrub_pos += span; }
        if self.scrub_pos >= loop_hi { self.scrub_pos -= span; }

        // Spawn scheduler.
        if self.sounding {
            let dens = density_g_per_sec.clamp(0.1, 80.0);
            self.spawn_acc += dens / self.sr;
            while self.spawn_acc >= 1.0 {
                self.spawn_acc -= 1.0;
                self.spawn(asset, size_ms, pitch_jit, pos_jit_ms, pan_jit, reverse);
            }
        }

        // Mix all active grains.
        let mut out_l = 0.0_f32;
        let mut out_r = 0.0_f32;
        for g in self.grains.iter_mut() {
            if !g.active { continue; }
            let s = read_interp(asset, g.pos);
            // Hann window: w(n) = 0.5 - 0.5 * cos_curr
            let win = 0.5 - 0.5 * g.cos_curr;
            out_l += s * win * g.gain_l;
            out_r += s * win * g.gain_r;
            // Advance cos via Chebyshev recurrence:
            // cos((n+1)*step) = 2*cos(step)*cos(n*step) - cos((n-1)*step)
            let next = g.cos_step * g.cos_curr - g.cos_prev;
            g.cos_prev = g.cos_curr;
            g.cos_curr = next;
            g.cos_phase += 1.0; // not used in math, just lifecycle
            // Advance position.
            g.pos += g.rate;
            // Wrap inside the asset (granular doesn't loop-respect
            // the asset's loop region — it can roam anywhere).
            let asset_len = asset.samples.len() as f64;
            if g.pos < 0.0 { g.pos += asset_len; }
            if g.pos >= asset_len { g.pos -= asset_len; }
            g.remaining = g.remaining.saturating_sub(1);
            if g.remaining == 0 { g.active = false; }
        }
        // Scale by 1/√N to keep output sane as density rises.
        let inv = 1.0 / (MAX_GRAINS as f32).sqrt();
        (out_l * inv, out_r * inv)
    }

    fn spawn(
        &mut self,
        asset:      &SampleAsset,
        size_ms:    f32,
        pitch_jit:  f32,
        pos_jit_ms: f32,
        pan_jit:    f32,
        reverse:    f32,
    ) {
        // Find an inactive slot.
        let slot = match self.grains.iter().position(|g| !g.active) {
            Some(i) => i,
            None => return, // pool exhausted; drop spawn
        };
        // Grain duration in engine samples.
        let dur_samples = ((size_ms.clamp(5.0, 1000.0)) * 0.001 * self.sr) as usize;
        let dur_samples = dur_samples.max(8);
        // Pitch jitter in semitones.
        let cents = (next_unit(&mut self.rng) - 0.5) * 2.0 * pitch_jit.clamp(0.0, 1.0) * 1200.0;
        let pitch_ratio = 2.0_f32.powf(cents / 1200.0);
        let maybe_reverse = if next_unit(&mut self.rng) < reverse.clamp(0.0, 1.0) {
            -1.0
        } else {
            1.0
        };
        let rate = (self.scrub_rate as f32 * pitch_ratio * maybe_reverse) as f64;
        // Position jitter (in asset samples).
        let pos_jit_samples = (pos_jit_ms.clamp(0.0, 500.0) * 0.001 * asset.sample_rate) as f64;
        let pos_off = (next_unit(&mut self.rng) as f64 - 0.5) * 2.0 * pos_jit_samples;
        let mut pos = self.scrub_pos + pos_off;
        let asset_len = asset.samples.len() as f64;
        if pos < 0.0 { pos += asset_len; }
        if pos >= asset_len { pos -= asset_len; }
        // Pan jitter.
        let pan = (next_unit(&mut self.rng) - 0.5) * 2.0 * pan_jit.clamp(0.0, 1.0);
        let pan_n = (pan + 1.0) * 0.5;
        let gain_l = (1.0 - pan_n).sqrt();
        let gain_r = pan_n.sqrt();
        // Initialize Hann recurrence.
        let n_total = dur_samples as f32;
        let step_rad = TAU / (n_total - 1.0).max(1.0);
        let cos_step = 2.0 * step_rad.cos();
        let cos_prev = (-step_rad).cos();   // cos(-step)
        let cos_curr = 1.0;                  // cos(0)

        self.grains[slot] = Grain {
            active: true,
            pos,
            rate,
            remaining: dur_samples,
            gain_l,
            gain_r,
            cos_phase: 0.0,
            cos_step,
            cos_prev,
            cos_curr,
        };
    }

    /// True when no grains are currently sounding (and no held note).
    pub fn is_idle(&self) -> bool {
        !self.sounding && self.grains.iter().all(|g| !g.active)
    }
}

#[inline(always)]
fn read_interp(asset: &SampleAsset, pos: f64) -> f32 {
    let n = asset.samples.len();
    if n == 0 { return 0.0; }
    let i0 = pos as usize % n;
    let i1 = (i0 + 1) % n;
    let frac = (pos - i0 as f64) as f32;
    let a = asset.samples[i0];
    let b = asset.samples[i1];
    a + (b - a) * frac
}

/// LCG -> uniform 0..1.
#[inline(always)]
fn next_unit(rng: &mut u64) -> f32 {
    *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*rng >> 33) as f32) / (1u64 << 31) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::samples::shared as samples_shared;

    fn first_asset() -> &'static SampleAsset {
        samples_shared().get(0).expect("global sample lib must have at least one asset")
    }

    #[test]
    fn idle_engine_produces_silence() {
        let asset = first_asset();
        let mut g = Granular::new(48_000.0);
        let mut peak = 0.0_f32;
        for _ in 0..4096 {
            let (l, r) = g.process(asset, 220.0, 20.0, 200.0, 0.1, 50.0, 0.5, 0.5, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak < 1e-6, "granular should be silent before note_on (peak {peak})");
    }

    #[test]
    fn note_on_then_density_produces_audio() {
        let asset = first_asset();
        let mut g = Granular::new(48_000.0);
        g.note_on(asset, 220.0);
        let mut peak = 0.0_f32;
        for _ in 0..8192 {
            let (l, r) = g.process(asset, 220.0, 30.0, 200.0, 0.05, 20.0, 0.3, 0.0, 0.0);
            peak = peak.max(l.abs()).max(r.abs());
        }
        assert!(peak > 0.05, "granular rendered silence at 30 g/s density (peak {peak})");
    }

    #[test]
    fn note_off_lets_grains_fade_then_idle() {
        let asset = first_asset();
        let mut g = Granular::new(48_000.0);
        g.note_on(asset, 220.0);
        for _ in 0..2048 {
            let _ = g.process(asset, 220.0, 30.0, 200.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        }
        g.note_off();
        for _ in 0..24_000 {
            let _ = g.process(asset, 220.0, 30.0, 200.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        }
        assert!(g.is_idle(), "granular should be idle after note_off + tail");
    }

    #[test]
    fn freeze_locks_scrub_position() {
        let asset = first_asset();
        let mut g = Granular::new(48_000.0);
        g.note_on(asset, 220.0);
        for _ in 0..512 {
            let _ = g.process(asset, 220.0, 5.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0);
        }
        let frozen_pos = g.scrub_pos;
        for _ in 0..2048 {
            let _ = g.process(asset, 220.0, 5.0, 50.0, 0.0, 0.0, 0.0, 1.0, 0.0);
        }
        assert!((g.scrub_pos - frozen_pos).abs() < 1e-3,
            "scrub_pos should be frozen with freeze=1.0 (drifted {})",
            g.scrub_pos - frozen_pos);
    }
}
