//! Wavetable phase warping (Serum-style), part B1 of the Pulsar upgrade.
//!
//! A warp remaps the read phase `[0,1)` BEFORE the single-cycle wavetable frame
//! is sampled, reshaping the waveform without changing its pitch. It is the
//! cheapest way to get the growl/vowel motion that defines a Serum patch: a
//! warp swept by an LFO makes one static frame sound like a whole wavetable.
//!
//! This module is the pure remap only: `warp_phase(mode, phase, depth) -> phase`.
//! It is intentionally UNWIRED — B1 injects it into `WavetableOsc::sample_at_phase`
//! once the real wavetable replaces the analytic morph, and re-baselines
//! `voice_render_is_byte_stable` on that day (a warp at rest is a bypass, so the
//! existing render is unaffected until a patch actually turns one on).
//!
//! CONTRACT (test-locked, no ear needed):
//!   - At its NEUTRAL depth every mode is an EXACT bypass: `warp_phase == phase`.
//!     Unipolar modes are neutral at `depth = 0`; the three bipolar modes
//!     (`BendBi`, `AsymBi`, and the folding `Mirror`/`Flip` are unipolar) are
//!     neutral at `depth = 0.5`, so an LFO sweeping `0..1` warps both ways around
//!     the centre. `is_bipolar()` names them.
//!   - The output always stays in `[0, 1)`, so the caller can read the frame
//!     without a second wrap.
//!   - Pure and deterministic: no state, no allocation, RT-safe.
//!
//! The curve SHAPES below are Serum-faithful starting points; their exact feel is
//! the one thing here that wants the ear, and gets tuned when B1 wires them.

use serde::{Deserialize, Serialize};

/// One entry per Serum warp mode. Positional order is the serialized/enum order —
/// append, never reorder (patches store the discriminant).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WarpMode {
    /// No remap. The rest-state of the oscillator.
    #[default]
    Off,
    /// Hard-sync feel: the cycle completes early and restarts within one period.
    SelfSync,
    /// Sync with the final partial cycle eased, taming the sync discontinuity.
    WindowedSync,
    /// Push phase energy toward the end of the cycle (a power curve, exp > 1).
    BendPlus,
    /// Push phase energy toward the start of the cycle (exp < 1).
    BendMinus,
    /// Bend either way; neutral at depth 0.5.
    BendBi,
    /// Pulse-width: move the cycle's midpoint, stretching one half.
    Pwm,
    /// Skew the waveform's symmetry toward the start.
    AsymPlus,
    /// Skew the waveform's symmetry toward the end.
    AsymMinus,
    /// Skew either way; neutral at depth 0.5.
    AsymBi,
    /// Fold the top fraction of the cycle back on itself.
    Flip,
    /// Read the cycle forward then backward (a triangle fold of phase).
    Mirror,
    /// Step-quantise the phase, so the redux artefact tracks the note.
    Quantize,
}

impl WarpMode {
    /// The 13 modes in enum order, for UI lists and coverage tests.
    pub const ALL: [WarpMode; 13] = [
        WarpMode::Off,
        WarpMode::SelfSync,
        WarpMode::WindowedSync,
        WarpMode::BendPlus,
        WarpMode::BendMinus,
        WarpMode::BendBi,
        WarpMode::Pwm,
        WarpMode::AsymPlus,
        WarpMode::AsymMinus,
        WarpMode::AsymBi,
        WarpMode::Flip,
        WarpMode::Mirror,
        WarpMode::Quantize,
    ];

    /// A short label for the editor's mode selector.
    pub fn label(self) -> &'static str {
        match self {
            WarpMode::Off => "Off",
            WarpMode::SelfSync => "Sync",
            WarpMode::WindowedSync => "Sync W",
            WarpMode::BendPlus => "Bend +",
            WarpMode::BendMinus => "Bend -",
            WarpMode::BendBi => "Bend +/-",
            WarpMode::Pwm => "PWM",
            WarpMode::AsymPlus => "Asym +",
            WarpMode::AsymMinus => "Asym -",
            WarpMode::AsymBi => "Asym +/-",
            WarpMode::Flip => "Flip",
            WarpMode::Mirror => "Mirror",
            WarpMode::Quantize => "Quantize",
        }
    }

    /// Bipolar modes are neutral at `depth = 0.5` (an LFO from 0..1 warps both
    /// ways around the centre); unipolar modes are neutral at `depth = 0`.
    pub fn is_bipolar(self) -> bool {
        matches!(self, WarpMode::BendBi | WarpMode::AsymBi)
    }

    /// The depth at which this mode is an exact bypass.
    pub fn neutral_depth(self) -> f32 {
        if self.is_bipolar() { 0.5 } else { 0.0 }
    }
}

// Curve ranges. Chosen so the extremes are strong but the phase map stays
// monotonic (or a clean fold), never a jump the reader can't follow.
const SYNC_MAX: f32 = 3.0; // up to 4 cycles inside one period
const BEND_MAX: f32 = 3.0; // power-curve exponent span
const ASYM_SKEW: f32 = 0.9; // parabolic skew gain (<1 keeps it monotonic)
const QUANT_MAX_STEPS: f32 = 64.0;

#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Parabolic symmetry skew: shifts the cycle's middle while pinning both ends,
/// so it stays monotonic for `|amount| < 1` and in-range. `amount = 0` is
/// identity; sign chooses the direction.
#[inline]
fn skew(p: f32, amount: f32) -> f32 {
    (p + amount * ASYM_SKEW * p * (1.0 - p)).clamp(0.0, ONE_MINUS_EPS)
}

const ONE_MINUS_EPS: f32 = 1.0 - f32::EPSILON;

/// Remap a read phase in `[0,1)` for the given warp mode and depth `[0,1]`.
///
/// Returns a phase in `[0,1)`. At the mode's neutral depth the return value is
/// exactly `phase` (see the module CONTRACT).
#[inline]
pub fn warp_phase(mode: WarpMode, phase: f32, depth: f32) -> f32 {
    let p = phase;
    let d = depth.clamp(0.0, 1.0);
    match mode {
        WarpMode::Off => p,

        WarpMode::SelfSync => {
            let k = 1.0 + d * SYNC_MAX;
            (p * k).fract()
        }

        WarpMode::WindowedSync => {
            let k = 1.0 + d * SYNC_MAX;
            let f = (p * k).fract();
            // Raised-cosine easing of the per-cycle ramp, faded in by depth so
            // depth 0 stays an exact bypass.
            let rc = 0.5 - 0.5 * (std::f32::consts::PI * f).cos();
            mix(f, rc, d)
        }

        WarpMode::BendPlus => p.powf(1.0 + d * BEND_MAX),
        WarpMode::BendMinus => p.powf(1.0 / (1.0 + d * BEND_MAX)),
        WarpMode::BendBi => {
            let b = (d - 0.5) * 2.0; // -1..1, 0 at neutral
            let e = if b >= 0.0 {
                1.0 + b * BEND_MAX
            } else {
                1.0 / (1.0 - b * BEND_MAX)
            };
            p.powf(e)
        }

        WarpMode::Pwm => {
            // Move the midpoint from 0.5 (bypass) toward 1.0, stretching the
            // first half of the cycle over more phase.
            let m = 0.5 + d * 0.49;
            if p < m {
                p * 0.5 / m
            } else {
                0.5 + (p - m) * 0.5 / (1.0 - m)
            }
        }

        WarpMode::AsymPlus => skew(p, d),
        WarpMode::AsymMinus => skew(p, -d),
        WarpMode::AsymBi => skew(p, (d - 0.5) * 2.0),

        WarpMode::Flip => {
            // Fold the top `d` fraction of the cycle back on itself.
            let t = 1.0 - d;
            if p <= t { p } else { (2.0 * t - p).max(0.0) }
        }

        WarpMode::Mirror => {
            let tri = if p < 0.5 { 2.0 * p } else { 2.0 * (1.0 - p) };
            mix(p, tri, d).clamp(0.0, ONE_MINUS_EPS)
        }

        WarpMode::Quantize => {
            if d <= f32::EPSILON {
                p // exact bypass; below this the step count is effectively infinite
            } else {
                // Fewer steps as depth rises: fine (near-transparent) -> coarse.
                let steps = mix(QUANT_MAX_STEPS, 2.0, d).round().max(2.0);
                (p * steps).floor() / steps
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A phase grid that stays strictly inside [0,1) as real read phases do.
    fn phases() -> Vec<f32> {
        (0..64).map(|i| i as f32 / 64.0).collect()
    }

    #[test]
    fn every_mode_is_an_exact_bypass_at_its_neutral_depth() {
        for mode in WarpMode::ALL {
            let d0 = mode.neutral_depth();
            for &p in &phases() {
                let out = warp_phase(mode, p, d0);
                assert!(
                    (out - p).abs() <= f32::EPSILON * 4.0,
                    "{:?} at neutral depth {d0} warped {p} -> {out}",
                    mode
                );
            }
        }
    }

    #[test]
    fn bipolar_modes_are_neutral_at_half_and_move_both_ways() {
        for mode in [WarpMode::BendBi, WarpMode::AsymBi] {
            assert!(mode.is_bipolar());
            // Neutral at 0.5 is covered above; here: 0 and 1 are non-trivial and
            // sit on OPPOSITE sides of the identity for a representative phase.
            let p = 0.3;
            let lo = warp_phase(mode, p, 0.0);
            let hi = warp_phase(mode, p, 1.0);
            assert!((lo - p).abs() > 1e-4, "{mode:?} depth 0 should warp");
            assert!((hi - p).abs() > 1e-4, "{mode:?} depth 1 should warp");
            assert!(
                (lo - p).signum() != (hi - p).signum(),
                "{mode:?} depth 0 ({lo}) and 1 ({hi}) should straddle identity {p}"
            );
        }
    }

    #[test]
    fn output_stays_in_unit_range_for_the_whole_grid() {
        for mode in WarpMode::ALL {
            for di in 0..=20 {
                let d = di as f32 / 20.0;
                for &p in &phases() {
                    let out = warp_phase(mode, p, d);
                    assert!(
                        out.is_finite() && (0.0..1.0).contains(&out),
                        "{mode:?} p={p} d={d} -> {out} out of [0,1)"
                    );
                }
            }
        }
    }

    #[test]
    fn unipolar_modes_actually_warp_at_full_depth() {
        // Guard against a mode accidentally being a no-op: at depth 1 every
        // unipolar mode (except Off) must change at least one phase.
        for mode in WarpMode::ALL {
            if mode == WarpMode::Off || mode.is_bipolar() {
                continue;
            }
            let moved = phases()
                .iter()
                .any(|&p| (warp_phase(mode, p, 1.0) - p).abs() > 1e-4);
            assert!(moved, "{mode:?} at depth 1 is a no-op");
        }
    }

    #[test]
    fn is_deterministic() {
        for mode in WarpMode::ALL {
            for &p in &phases() {
                assert_eq!(warp_phase(mode, p, 0.37), warp_phase(mode, p, 0.37));
            }
        }
    }

    #[test]
    fn labels_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for mode in WarpMode::ALL {
            assert!(seen.insert(mode.label()), "duplicate label {}", mode.label());
        }
    }
}
