//! The chain a patch describes, and the space each factory preset is heard in.
//!
//! Described here, run nowhere: the engine owns no effects. The host builds
//! a live chain from this description.
//!
//! Three effects in a fixed order: an equaliser, a space, a width. Which
//! ones and in which order is not a preset's business; what each is set to
//! is. The equaliser ships flat, every band off: the bodies are calibrated
//! on recordings and want no correction, so the bands are the player's. The
//! space is what the model lacks: a string radiates into a room, and the
//! model has no walls. The width is where the listener sits: a soloist
//! close and narrow, a section across the stage, the low strings mono
//! below the bass where a wide image only blurs. The figures are a
//! production choice, made by ear, and say so.

use phonix_fx::{ChainSpec, SlotSpec};

use crate::patch::ArchetPatch;

/// Slots the chain occupies. Frozen with the order below.
pub const FX_SLOTS: usize = 3;

/// The space a preset is heard in.
#[derive(Clone, Copy, Debug)]
pub struct Space {
    /// The reverb's type, by its id.
    pub kind: &'static str,
    /// 0..1. How big the space is.
    pub size: f32,
    /// 0..1, not seconds: the reverb maps it per type.
    pub decay: f32,
    /// 0..1. How much of the high end the walls absorb.
    pub damping: f32,
    /// Seconds before the first reflection.
    pub predelay: f32,
    /// 0..1. How much of it is heard: the slot's mix.
    pub mix: f32,
    /// 0..2. The stereo width the listener hears, one being the seats as
    /// the engine lays them.
    pub width: f32,
}

/// Below this the image is mono: a low string's fundamental gains nothing
/// from width.
pub const MONO_BELOW_HZ: f32 = 120.0;

/// A soloist a few metres away in a small hall.
pub const CHAMBER: Space =
    Space { kind: "hall", size: 0.30, decay: 0.35, damping: 0.50, predelay: 0.012, mix: 0.18, width: 0.8 };
/// A section on a stage.
pub const HALL: Space =
    Space { kind: "hall", size: 0.55, decay: 0.50, damping: 0.50, predelay: 0.020, mix: 0.28, width: 1.2 };
/// A full string body in a concert hall.
pub const CONCERT: Space =
    Space { kind: "hall", size: 0.70, decay: 0.60, damping: 0.45, predelay: 0.025, mix: 0.32, width: 1.3 };
/// Plucked strings close by, in a room that lets each pluck stay distinct.
pub const ROOM: Space =
    Space { kind: "room", size: 0.25, decay: 0.30, damping: 0.60, predelay: 0.008, mix: 0.12, width: 1.0 };

/// Build the chain. Units are the effects' own: the EQ in Hz and dB, the
/// reverb normalised except a pre-delay in seconds, the width as a factor
/// and its mono corner in Hz.
pub fn chain(space: Space) -> ChainSpec {
    ChainSpec::new(vec![
        // Flat: the bands are there for the player, none is engaged.
        SlotSpec::new("parametric-eq")
            .with("band.0.enabled", false)
            .with("band.1.enabled", false)
            .with("band.2.enabled", false)
            .with("band.3.enabled", false),
        SlotSpec::new("reverb")
            .with("type", space.kind)
            .with("size", space.size)
            .with("decay", space.decay)
            .with("damping", space.damping)
            .with("predelay", space.predelay)
            .with("width", 1.0_f32)
            .mix(space.mix),
        SlotSpec::new("stereo-imager")
            .with("width", space.width)
            .with("mono-freq", MONO_BELOW_HZ),
    ])
}

/// The space that fits what a patch plays: plucked strings in a room, a
/// whole string body in a concert hall, a section in a hall, a soloist in
/// a chamber.
pub fn space_for(p: &ArchetPatch) -> Space {
    if p.pluck {
        ROOM
    } else if p.auto_range {
        CONCERT
    } else if p.ensemble >= 1.5 {
        HALL
    } else {
        CHAMBER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind and parameter the recipe names exists in the build, in
    /// range, under the name written.
    #[test]
    fn the_recipe_names_only_what_the_build_has() {
        for space in [CHAMBER, HALL, CONCERT, ROOM] {
            let report = chain(space).check(&phonix_fx::Registry::builtin());
            assert!(report.is_clean(), "{report}");
        }
    }

    #[test]
    fn the_order_and_the_kinds_are_frozen() {
        let spec = chain(CHAMBER);
        assert_eq!(spec.kinds().collect::<Vec<_>>(), ["parametric-eq", "reverb", "stereo-imager"]);
        assert_eq!(spec.len(), FX_SLOTS);
        assert!(spec.slots.iter().all(|s| s.enabled));
    }

    #[test]
    fn every_factory_preset_carries_its_own_chain() {
        for p in ArchetPatch::factory_presets() {
            assert_eq!(p.fx.len(), FX_SLOTS, "{} carries no chain", p.name);
            let kind = match p.fx.slots[1].get("type") {
                Some(phonix_fx::SpecValue::S(v)) => v.as_str(),
                _ => "",
            };
            let want = space_for(&p).kind;
            assert_eq!(kind, want, "{} is heard in the wrong space", p.name);
        }
    }

    #[test]
    fn the_default_patch_is_a_soloist_in_a_chamber() {
        assert_eq!(ArchetPatch::default().fx, chain(CHAMBER));
    }

    /// A patch written before `fx` existed deserialises to an empty chain,
    /// and an empty chain is a real no-op.
    #[test]
    fn a_patch_written_before_the_chain_still_loads() {
        let mut v: serde_json::Value = serde_json::to_value(ArchetPatch::default()).unwrap();
        v.as_object_mut().unwrap().remove("fx");
        let p: ArchetPatch = serde_json::from_value(v).unwrap();
        assert!(p.fx.is_empty());
    }
}
