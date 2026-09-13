//! A fingerprint of rendered audio that pins its sound, not its bits.
//!
//! Two machines round the library's transcendental functions differently
//! in the last bit, so a hash of the samples cannot be compared across
//! them. This one hashes, per block, the level in half-decibel steps and
//! the number of zero crossings: a last-bit difference moves neither, and
//! any change a listener could hear moves both.

const BLOCK: usize = 1024;
const STEP_DB: f32 = 0.5;
const FLOOR_DB: f32 = -120.0;

/// The fingerprint of `samples`, one channel.
pub fn of(samples: &[f32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        for byte in v.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for block in samples.chunks(BLOCK) {
        let power = block.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / block.len() as f64;
        let db = (10.0 * power.max(1e-30).log10() as f32).max(FLOOR_DB);
        let level = (db / STEP_DB).round() as i64;
        let crossings = block.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as u64;
        mix(level as u64);
        mix(crossings);
    }
    h
}
