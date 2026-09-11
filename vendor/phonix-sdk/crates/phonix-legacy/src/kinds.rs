//! The historical effect list, by ordinal. Frozen: the ordinal is what
//! every old session and patch wrote.

/// Index 0 is the empty slot.
pub const KINDS: [&str; 51] = [
    "",
    "delay",
    "reverb",
    "chorus",
    "phaser",
    "flanger",
    "distortion",
    "vocoder",
    "compressor",
    "limiter",
    "expander",
    "auto-panner",
    "stutter",
    "filter",
    "ring-mod",
    "lo-fi",
    "stereo-imager",
    "choir",
    "maximizer",
    "spatializer-3d",
    "parametric-eq",
    "doppler",
    "transient-shaper",
    "granular-freezer",
    "spectral-freeze",
    "resonator-bank",
    "auto-wah",
    "tape-stop",
    "brickwall-limiter",
    "lufs-meter",
    "dynamic-eq",
    "mid-side-eq",
    "multiband-compressor",
    "spectrum-analyzer",
    "master-assistant",
    "convolution-reverb",
    "multiband-ducker",
    "velvet-decorrelator",
    "analog-drift",
    "early-reflections",
    "mid-side-widen",
    "haas-split",
    "retune",
    "harmony",
    "trance-gate",
    "rack",
    "ensemble",
    "spring-tank",
    "plate-hall",
    "schroeder-reverb",
    "dimension-chorus",
];

/// The kind an old ordinal names; `None` for the empty slot and for an
/// ordinal past the list.
pub fn kind_of(ordinal: u8) -> Option<&'static str> {
    match KINDS.get(ordinal as usize) {
        Some(k) if !k.is_empty() => Some(k),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ordinal_names_one_kebab_case_kind_once() {
        for (i, k) in KINDS.iter().enumerate().skip(1) {
            assert!(!k.is_empty(), "ordinal {i} is unnamed");
            assert!(k.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "{k}");
            assert!(!KINDS[..i].contains(k), "{k} is listed twice");
        }
        assert_eq!(kind_of(0), None);
        assert_eq!(kind_of(20), Some("parametric-eq"));
        assert_eq!(kind_of(45), Some("rack"));
        assert_eq!(kind_of(51), None);
    }
}
