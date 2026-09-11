//! Shared fast transcendental approximations for hot per-sample DSP loops.
//!
//! These trade a few 1e-3 of accuracy for a large speedup over libm, and are
//! used to replace per-sample `tanh`/`exp2` in inner audio loops (waveshapers,
//! filter feedback soft-clip, pitch). Every use is gated by the campaign's
//! null-test, since they slightly change the rendered output (sub-perceptual).
//!
//! Also provides `fast_sin` (a 4096-entry table with linear interpolation)
//! alongside the saturating + exponential approximations.

/// tanh approximation, monotonic and saturating, max error ~2.4e-2 over the
/// usable range. At |x| = 3 it reaches exactly +-1, so clamping there gives a
/// clean saturating soft-clip. Accuracy is plenty for waveshaping / filter
/// feedback (a gentle nonlinearity, not a precision operation); do NOT use it
/// where the exact tanh value matters numerically.
#[inline(always)]
pub fn fast_tanh(x: f32) -> f32 {
    let x = x.clamp(-3.0, 3.0);
    let x2 = x * x;
    x * (27.0 + x2) / (27.0 + 9.0 * x2)
}

/// Accurate sine via a 4096-entry table over one period with linear
/// interpolation.
const SINE_N: usize = 4096;
static SINE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
#[inline]
fn sine_table() -> &'static [f32] {
    SINE.get_or_init(|| (0..=SINE_N)
        .map(|i| (i as f32 / SINE_N as f32 * std::f32::consts::TAU).sin())
        .collect())
}
#[inline(always)]
pub fn fast_sin(x: f32) -> f32 {
    let t = sine_table();
    let mut p = x * (1.0 / std::f32::consts::TAU);
    p -= p.floor();
    let idx = p * SINE_N as f32;
    let i0 = idx as usize;
    let frac = idx - i0 as f32;
    t[i0] + (t[i0 + 1] - t[i0]) * frac
}

/// High-accuracy tanh via a 2048-entry lookup table over [-6, 6] with linear
/// interpolation (max error ~3e-6, saturating exactly to +-1 beyond the range).
/// Unlike `fast_tanh` (2.4e-2 error) this PRESERVES harmonic structure through a
/// high-gain cascade, so it is safe to replace `tanh` in a multi-stage waveshaper
/// where the per-stage shape matters (e.g. the guitar tube cascade). ~5x cheaper
/// than libm `tanh`. Fetch the table once with `tanh_table()` and pass it to
/// `tanh_lut` so the per-sample inner loop avoids the OnceLock load.
const TANH_N: usize = 2048;
const TANH_RANGE: f32 = 6.0;
// HEAP-backed (Vec), NOT an inline [f32; 2049]: the table is built inside an engine
// constructor (AmpSim::new), itself called deep in TrackEngine construction; an 8 KB
// array built on the stack there overflowed it. The Vec builds on the heap.
static TANH_LUT: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();

/// The tanh table (built on first use; do this during warmup, not on a hot path).
pub fn tanh_table() -> &'static [f32] {
    TANH_LUT.get_or_init(|| {
        let mut t = vec![0.0f32; TANH_N + 1];
        for (i, e) in t.iter_mut().enumerate() {
            let x = (i as f32 / TANH_N as f32) * (2.0 * TANH_RANGE) - TANH_RANGE;
            *e = x.tanh();
        }
        t
    })
}

/// Look up tanh(x) in a table fetched via `tanh_table()` (passed as a slice).
#[inline(always)]
pub fn tanh_lut(x: f32, t: &[f32]) -> f32 {
    let xc = x.clamp(-TANH_RANGE, TANH_RANGE);
    let pos = (xc + TANH_RANGE) * (TANH_N as f32 / (2.0 * TANH_RANGE));
    let i = (pos as usize).min(TANH_N - 1);
    let frac = pos - i as f32;
    t[i] + (t[i + 1] - t[i]) * frac
}

/// 2^x for x in a moderate range (pitch ratios from semitone offsets). Splits
/// into an integer power-of-two (exact, via bit-twiddling on the exponent) and a
/// degree-3 polynomial for the fractional mantissa. Accurate to ~3e-4, far
/// cheaper than libm `exp2`/`powf`. Used for per-sample vibrato/detune pitch.
#[inline(always)]
pub fn fast_exp2(x: f32) -> f32 {
    // Clamp to a sane musical range to keep the polynomial accurate and avoid
    // exponent overflow (covers +-10 octaves).
    let x = x.clamp(-10.0, 10.0);
    let xi = x.floor();
    let xf = x - xi; // [0,1)
    // Degree-3 minimax-ish poly for 2^xf on [0,1].
    let p = 1.0 + xf * (0.6960656421 + xf * (0.2240734647 + xf * 0.0792018));
    // Scale by 2^xi via direct exponent construction.
    let exp_bits = ((xi as i32) + 127) as u32;
    let pow2i = f32::from_bits(exp_bits << 23);
    p * pow2i
}

/// `exp(-x)` for x >= 0 (decay coefficients), via fast_exp2 (e^-x = 2^(-x*log2e)).
#[inline(always)]
pub fn fast_exp_neg(x: f32) -> f32 {
    const LOG2E: f32 = 1.442695041;
    fast_exp2(-x * LOG2E)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_tanh_accuracy() {
        let mut max_err = 0.0f32;
        let mut x = -4.0f32;
        while x <= 4.0 {
            max_err = max_err.max((fast_tanh(x) - x.tanh()).abs());
            x += 0.01;
        }
        assert!(max_err < 3e-2, "fast_tanh max err {max_err}");
    }

    #[test]
    fn tanh_lut_accuracy() {
        let t = tanh_table();
        let mut max_err = 0.0f32;
        let mut x = -8.0f32;
        while x <= 8.0 {
            max_err = max_err.max((tanh_lut(x, t) - x.tanh()).abs());
            x += 0.005;
        }
        assert!(max_err < 1e-4, "tanh_lut max err {max_err}");
    }

    #[test]
    fn fast_exp2_accuracy() {
        let mut max_rel = 0.0f32;
        let mut x = -10.0f32;
        while x <= 10.0 {
            let r = ((fast_exp2(x) - x.exp2()) / x.exp2()).abs();
            max_rel = max_rel.max(r);
            x += 0.013;
        }
        assert!(max_rel < 2e-3, "fast_exp2 max rel err {max_rel}");
    }
}
