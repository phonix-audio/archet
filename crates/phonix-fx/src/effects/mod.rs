//! The effects this build was compiled with, one module per feature.

#[cfg(any(feature = "compressor", feature = "reverb", feature = "delay"))]
mod math;
#[cfg(any(feature = "reverb", feature = "delay"))]
mod lines;
#[cfg(feature = "delay")]
pub mod tempo;

#[cfg(feature = "brickwall-limiter")]
pub mod brickwall_limiter;
#[cfg(feature = "compressor")]
pub mod compressor;
#[cfg(feature = "delay")]
pub mod delay;
#[cfg(feature = "parametric-eq")]
pub mod parametric_eq;
#[cfg(feature = "reverb")]
pub mod reverb;
#[cfg(feature = "stereo-imager")]
pub mod stereo_imager;

use crate::registry::Entry;

/// Every kind compiled in, in a fixed order.
pub fn builtin() -> Vec<Entry> {
    #[allow(unused_mut)]
    let mut entries: Vec<Entry> = Vec::new();
    #[cfg(feature = "parametric-eq")]
    entries.push(Entry { spec: &parametric_eq::SPEC, build: parametric_eq::build });
    #[cfg(feature = "compressor")]
    entries.push(Entry { spec: &compressor::SPEC, build: compressor::build });
    #[cfg(feature = "reverb")]
    entries.push(Entry { spec: &reverb::SPEC, build: reverb::build });
    #[cfg(feature = "brickwall-limiter")]
    entries.push(Entry { spec: &brickwall_limiter::SPEC, build: brickwall_limiter::build });
    #[cfg(feature = "delay")]
    entries.push(Entry { spec: &delay::SPEC, build: delay::build });
    #[cfg(feature = "stereo-imager")]
    entries.push(Entry { spec: &stereo_imager::SPEC, build: stereo_imager::build });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every compiled kind describes itself without a fault, and no two
    /// share a name.
    #[test]
    fn every_builtin_spec_is_sound_and_unique() {
        let entries = builtin();
        for (i, e) in entries.iter().enumerate() {
            assert!(e.spec.problems().is_empty(), "{}: {:?}", e.spec.kind, e.spec.problems());
            assert!(!entries[..i].iter().any(|o| o.spec.kind == e.spec.kind), "{} twice", e.spec.kind);
        }
    }

    /// A kind built at one rate, moved to another and given its
    /// parameters back reads the same parameters.
    #[test]
    fn every_builtin_survives_a_sample_rate_change_with_its_parameters() {
        for e in builtin() {
            let mut fx = (e.build)(44_100.0);
            let values: Vec<_> = e.spec.params.iter().map(|p| p.default).collect();
            fx.set_sample_rate(96_000.0);
            for (i, v) in values.iter().enumerate() {
                fx.set_param(i, *v);
            }
            for (i, p) in e.spec.params.iter().enumerate() {
                let got = fx.param(i);
                assert!(p.accepts(got), "{}.{} reads {:?} outside its range", e.spec.kind, p.id, got);
            }
        }
    }
}
