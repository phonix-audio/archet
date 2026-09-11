//! The old parameter space, named. `(kind, pid)` is the key: the same pid
//! meant different things under different kinds.

/// How an old `f32` becomes a written value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rule {
    Direct,
    Scale(f32),
    Bool,
    Int,
    /// An ordinal into these variants.
    Enum(&'static [&'static str]),
    /// `n` bits of one integer, each becoming the bool whose id is the
    /// row's id with `{}` replaced by the bit's index.
    BitsToBools(u8),
    /// The value was the slot's mix, stored as a parameter.
    IntoSlotMix,
}

/// One row: the kind, the old pid, the new id and how the value converts.
pub struct Row {
    pub kind: &'static str,
    pub pid: u8,
    pub id: &'static str,
    pub rule: Rule,
}

/// Grows as each kind is ported; a kind with no rows converts to its name
/// alone and every pid it carried is reported.
pub static PARAMS: &[Row] = &[
    // parametric-eq
    Row { kind: "parametric-eq", pid: 28, id: "band.0.freq", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 29, id: "band.0.gain", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 38, id: "band.0.q", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 39, id: "band.1.freq", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 48, id: "band.1.gain", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 49, id: "band.1.q", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 57, id: "band.2.freq", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 58, id: "band.2.gain", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 59, id: "band.2.q", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 68, id: "band.3.freq", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 69, id: "band.3.gain", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 78, id: "band.3.q", rule: Rule::Direct },
    Row { kind: "parametric-eq", pid: 79, id: "", rule: Rule::IntoSlotMix },
    Row { kind: "parametric-eq", pid: 80, id: "band.{}.enabled", rule: Rule::BitsToBools(4) },
    Row { kind: "parametric-eq", pid: 81, id: "band.0.type", rule: Rule::Enum(&["auto", "peak", "low-shelf", "high-shelf", "low-pass"]) },
    Row { kind: "parametric-eq", pid: 82, id: "band.1.type", rule: Rule::Enum(&["auto", "peak", "low-shelf", "high-shelf", "low-pass"]) },
    Row { kind: "parametric-eq", pid: 83, id: "band.2.type", rule: Rule::Enum(&["auto", "peak", "low-shelf", "high-shelf", "low-pass"]) },
    Row { kind: "parametric-eq", pid: 84, id: "band.3.type", rule: Rule::Enum(&["auto", "peak", "low-shelf", "high-shelf", "low-pass"]) },
    // compressor
    Row { kind: "compressor", pid: 60, id: "threshold", rule: Rule::Direct },
    Row { kind: "compressor", pid: 61, id: "ratio", rule: Rule::Direct },
    Row { kind: "compressor", pid: 62, id: "attack", rule: Rule::Direct },
    Row { kind: "compressor", pid: 63, id: "release", rule: Rule::Direct },
    Row { kind: "compressor", pid: 64, id: "makeup", rule: Rule::Direct },
    Row { kind: "compressor", pid: 65, id: "knee", rule: Rule::Direct },
    Row { kind: "compressor", pid: 66, id: "", rule: Rule::IntoSlotMix },
    Row { kind: "compressor", pid: 67, id: "mode", rule: Rule::Enum(&["vca", "opto", "fet", "multiband", "bus"]) },
    Row { kind: "compressor", pid: 68, id: "sidechain-hpf", rule: Rule::Direct },
    Row { kind: "compressor", pid: 69, id: "auto-makeup", rule: Rule::Direct },
    // reverb
    Row { kind: "reverb", pid: 10, id: "size", rule: Rule::Direct },
    Row { kind: "reverb", pid: 11, id: "decay", rule: Rule::Direct },
    Row { kind: "reverb", pid: 12, id: "damping", rule: Rule::Direct },
    Row { kind: "reverb", pid: 13, id: "predelay", rule: Rule::Direct },
    Row { kind: "reverb", pid: 14, id: "", rule: Rule::IntoSlotMix },
    Row { kind: "reverb", pid: 15, id: "width", rule: Rule::Direct },
    Row { kind: "reverb", pid: 16, id: "type", rule: Rule::Enum(&["hall", "room", "plate", "spring", "shimmer", "cathedral", "cave", "ambient", "gated", "reverse", "infinite"]) },
    // delay
    Row { kind: "delay", pid: 0, id: "time", rule: Rule::Direct },
    Row { kind: "delay", pid: 1, id: "feedback", rule: Rule::Direct },
    Row { kind: "delay", pid: 2, id: "level", rule: Rule::Direct },
    Row { kind: "delay", pid: 3, id: "stereo", rule: Rule::Direct },
    Row { kind: "delay", pid: 4, id: "high-cut", rule: Rule::Direct },
    Row { kind: "delay", pid: 5, id: "low-cut", rule: Rule::Direct },
    Row { kind: "delay", pid: 6, id: "sync", rule: Rule::Bool },
    Row { kind: "delay", pid: 7, id: "division", rule: Rule::Enum(&["1-1", "1-2", "1-4", "1-8", "1-16", "1-32", "1-2d", "1-4d", "1-8d", "1-16d", "1-2t", "1-4t", "1-8t", "1-16t"]) },
    Row { kind: "delay", pid: 8, id: "type", rule: Rule::Enum(&["digital", "ping-pong", "tape", "multi-tap", "slapback", "analog", "reverse", "ducking", "diffused", "shimmer", "lo-fi"]) },
    // stereo-imager
    Row { kind: "stereo-imager", pid: 28, id: "width", rule: Rule::Direct },
    Row { kind: "stereo-imager", pid: 29, id: "mono-freq", rule: Rule::Direct },
    Row { kind: "stereo-imager", pid: 38, id: "", rule: Rule::IntoSlotMix },
    // brickwall-limiter
    Row { kind: "brickwall-limiter", pid: 28, id: "ceiling", rule: Rule::Direct },
    Row { kind: "brickwall-limiter", pid: 29, id: "release", rule: Rule::Direct },
    Row { kind: "brickwall-limiter", pid: 38, id: "", rule: Rule::IntoSlotMix },
];

pub fn lookup(kind: &str, pid: u8) -> Option<(&'static str, Rule)> {
    PARAMS.iter().find(|r| r.kind == kind && r.pid == pid).map(|r| (r.id, r.rule))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_pair_is_listed_twice_and_every_kind_is_in_the_list() {
        for (i, r) in PARAMS.iter().enumerate() {
            assert!(crate::KINDS.contains(&r.kind), "{} is not a historical kind", r.kind);
            assert!(!PARAMS[..i].iter().any(|q| q.kind == r.kind && q.pid == r.pid), "({}, {}) twice", r.kind, r.pid);
        }
    }
}
