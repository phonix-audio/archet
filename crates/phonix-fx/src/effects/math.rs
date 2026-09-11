//! The approximations the ported effects were written against; kept as
//! they were so a render does not move.

/// Fast dB-to-linear conversion using exp2 approximation.
/// 10^(db/20) = 2^(db * log2(10) / 20) = 2^(db * 0.16609640...)
/// Uses the identity: exp2(x) ≈ exp(x * ln2), which the compiler can optimize well.
#[cfg(feature = "compressor")]
#[inline(always)]
pub(super) fn fast_db_to_lin(db: f32) -> f32 {
    // 10^(db/20) = e^(db * ln(10)/20)
    (db * (std::f32::consts::LN_10 / 20.0)).exp()
}

/// Fast log10 approximation using IEEE754 float bit tricks.
/// ~0.5% accuracy, avoids expensive libm log10f. For dB conversion in compressors.
#[cfg(feature = "compressor")]
#[inline(always)]
pub(super) fn fast_log10(x: f32) -> f32 {
    // log10(x) = log2(x) / log2(10)
    // log2(x) ≈ bit manipulation on IEEE754 floats
    let i = x.to_bits() as f32;
    let log2 = i * (1.0 / (1 << 23) as f32) - 127.0;
    log2 * std::f32::consts::LOG10_2
}

/// Fast polynomial sin approximation for LFOs (±3% accuracy, no branching).
/// Input in radians. Good enough for modulation where precision doesn't matter.
#[cfg(any(feature = "reverb", feature = "delay"))]
#[inline(always)]
pub(super) fn fast_sin(x: f32) -> f32 {
    // Normalize to [-pi, pi]
    let mut x = x % std::f32::consts::TAU;
    if x > std::f32::consts::PI {
        x -= std::f32::consts::TAU;
    }
    if x < -std::f32::consts::PI {
        x += std::f32::consts::TAU;
    }
    // Bhaskara I approximation: 16x(pi-x) / (5pi^2 - 4x(pi-x))
    let pi = std::f32::consts::PI;
    let abs_x = x.abs();
    let y = x * (pi - abs_x);
    let denom = 5.0 * pi * pi - 4.0 * y.abs();
    16.0 * y / denom
}
