//! Archet patch - all tunable parameters of the bowed-string model.
//!
//! Every field carries `#[serde(default)]` so older saved patches load forward.
//! Values are the literature-derived starting points (see module docs and the
//! plan's Sources); they are exposed so the de-risk and presets can tune them.

use serde::{Deserialize, Serialize};
use super::friction::FrictionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum Instrument {
    #[default]
    Violin,
    Viola,
    Cello,
    DoubleBass,
}

/// One editable field of an [`ArchetPatch`], for `ArchetCommand::SetParam`.
///
/// Archet had exactly two typed setters (`SetPolyphony`, `SetOutputLevel`);
/// every other knob pushed a whole patch through `LoadPatch`. That is not just
/// wasteful here: `load_patch` rebuilds the sympathetic strings and re-seeds
/// the voices, and the engine already carries two defensive guards written
/// specifically because "the editor pushes a whole patch on every knob edit".
/// Naming the field removes the reason those guards exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchetParam {
    BowPos, BowVel, BowForce, BowNoise,
    Loss, Slope, Friction, BridgeHillDb,
    TorRatio, TorCouple, TorInject,
    Attack, Release, VelSens,
    VibRate, VibDepth, VibDelay,
    Ensemble, TuneCents,
    Instrument, AutoRange, Pluck,
}

impl ArchetParam {
    /// Does changing this field require rebuilding the sympathetic open
    /// strings? Only the body choice does; every other field is a coefficient
    /// the running voices read each block.
    pub fn needs_symp_rebuild(self) -> bool {
        matches!(self, ArchetParam::Instrument | ArchetParam::AutoRange | ArchetParam::Pluck)
    }

    /// Write `value` into `p`. Enums and bools travel as an f32 index, the same
    /// wire format the declarative GUI uses for every other engine.
    pub fn apply(self, p: &mut ArchetPatch, value: f32) {
        use ArchetParam as P;
        match self {
            P::BowPos       => p.bow_pos = value,
            P::BowVel       => p.bow_vel = value,
            P::BowForce     => p.bow_force = value,
            P::BowNoise     => p.bow_noise = value,
            P::Loss         => p.loss = value,
            P::Slope        => p.slope = value,
            P::BridgeHillDb => p.bridge_hill_db = value,
            P::TorRatio     => p.tor_ratio = value,
            P::TorCouple    => p.tor_couple = value,
            P::TorInject    => p.tor_inject = value,
            P::Attack       => p.attack = value,
            P::Release      => p.release = value,
            P::VelSens      => p.vel_sens = value,
            P::VibRate      => p.vib_rate = value,
            P::VibDepth     => p.vib_depth = value,
            P::VibDelay     => p.vib_delay = value,
            P::Ensemble     => p.ensemble = value,
            P::TuneCents    => p.tune_cents = value,
            P::Friction     => {
                p.friction = if value >= 0.5 { FrictionKind::ElastoPlastic }
                             else { FrictionKind::Static };
            }
            P::Instrument   => p.instrument = Instrument::from_index(value as usize),
            P::AutoRange    => p.auto_range = value >= 0.5,
            P::Pluck        => p.pluck = value >= 0.5,
        }
    }
}

impl Instrument {
    /// Index tables. Written out because this is the GUI/audio wire format:
    /// the declarative layout stores the choice as a number, so the order is
    /// data, not an implementation detail of the enum.
    pub const ALL_ORDERED: [Instrument; 4] = [
        Instrument::Violin, Instrument::Viola, Instrument::Cello, Instrument::DoubleBass,
    ];
    pub fn from_index(i: usize) -> Instrument {
        Self::ALL_ORDERED[i.min(Self::ALL_ORDERED.len() - 1)]
    }
    pub fn to_index(self) -> usize {
        Self::ALL_ORDERED.iter().position(|&x| x == self).unwrap_or(0)
    }
}


impl Instrument {
    /// Inverse-body-length body-mode frequency scale (Gough 2016 family).
    pub fn body_scale(self) -> f32 {
        match self {
            Instrument::Violin => 1.0,
            Instrument::Viola => 0.82,
            Instrument::Cello => 0.50,
            Instrument::DoubleBass => 0.33,
        }
    }
    /// Index into the per-instrument body curves in body.rs.
    pub fn body_index(self) -> usize {
        match self {
            Instrument::Violin => 0,
            Instrument::Viola => 1,
            Instrument::Cello => 2,
            Instrument::DoubleBass => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
pub enum FrictionKind {
    Static,
    #[default]
    ElastoPlastic,
}


impl From<FrictionKind> for FrictionMode {
    fn from(k: FrictionKind) -> Self {
        match k {
            FrictionKind::Static => FrictionMode::Static,
            FrictionKind::ElastoPlastic => FrictionMode::ElastoPlastic,
        }
    }
}

fn def_poly() -> u8 {
    8
}
fn def_one() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchetPatch {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub instrument: Instrument,
    /// Full-range ensemble mode: pick the instrument body per note from its pitch
    /// (violin/viola/cello/bass) instead of one fixed instrument. Lets a single engine
    /// render a composite string desk (a GM String-Ensemble track spans the whole
    /// choir) -- so the render is ONE engine per MIDI track, not one per pitch-band.
    #[serde(default)]
    pub auto_range: bool,
    /// Plucked articulation (pizzicato): a one-shot fingertip excitation then a
    /// free modal ring-down -- no bow, no gesture, no vibrato. Nothing damps it on
    /// note-off, because a finger leaves the string and the note decays by its own
    /// losses. The force follows the velocity: a finger plucks harder or softer.
    #[serde(default)]
    pub pluck: bool,
    #[serde(default = "def_poly")]
    pub polyphony: u8,

    // --- bow / string ---
    #[serde(default)]
    pub friction: FrictionKind,
    /// Bow position beta (bow-bridge distance / length), ~0.05..0.12.
    #[serde(default)]
    pub bow_pos: f32,
    /// Sustained bow velocity (loudness/brightness).
    #[serde(default)]
    pub bow_vel: f32,
    /// Bow force (Schelleng): more force => brighter/louder within the window.
    #[serde(default)]
    pub bow_force: f32,
    /// Bridge loss one-pole coefficient (0 bright .. 1 dark).
    #[serde(default)]
    pub loss: f32,
    /// Static-table slope (only used when friction == Static).
    #[serde(default)]
    pub slope: f32,

    // --- torsion ---
    #[serde(default)]
    pub tor_ratio: f32,
    #[serde(default)]
    pub tor_couple: f32,
    #[serde(default)]
    pub tor_inject: f32,

    // --- body ---
    /// Bridge-hill presence in dB (the brightness control of the body).
    #[serde(default)]
    pub bridge_hill_db: f32,

    // --- bow noise ---
    #[serde(default)]
    pub bow_noise: f32,

    // --- attack / articulation ---
    /// Bow-force ramp time at note-on (s) - the pre-Helmholtz onset.
    #[serde(default)]
    pub attack: f32,
    /// Bow-lift ramp time at note-off (s).
    #[serde(default)]
    pub release: f32,

    // --- vibrato ---
    #[serde(default)]
    pub vib_rate: f32, // Hz
    #[serde(default)]
    pub vib_depth: f32, // cents
    /// Seconds of note before vibrato fades in (delayed vibrato).
    #[serde(default)]
    pub vib_delay: f32,

    // --- output ---
    #[serde(default = "def_one")]
    pub output_level: f32,
    /// Velocity -> bow force/level sensitivity.
    #[serde(default)]
    pub vel_sens: f32,
    /// Overall tuning offset in cents (per-desk detune so a section of these is
    /// many slightly-different instruments, not phased clones).
    #[serde(default)]
    pub tune_cents: f32,
    /// Per-player decorrelation seed for string sections. Each desk in an
    /// ensemble loads the same patch with a distinct `seed_offset`, which
    /// re-seeds every noise, micro-pitch and vibrato stream so the players
    /// are independent and their partials do not phase-lock. 0 is the
    /// default single voice.
    #[serde(default)]
    pub seed_offset: u32,
    /// String-section size, in players (0 or 1 is solo). The engine renders
    /// a bounded pool of decorrelated physical voices (with the 14-cent F0
    /// scatter Ternstroem measures) and, above that pool, a diffuser of
    /// constant cost adds the rest of the section (section.rs).
    #[serde(default)]
    pub ensemble: f32,
    /// The effects the host runs after the engine: an equaliser, a space,
    /// a ceiling, described here and run nowhere in the engine. Appended,
    /// and empty by default, so a patch written before it existed keeps
    /// sounding as it did: an empty chain is a real no-op.
    #[serde(default)]
    pub fx: phonix_fx::ChainSpec,
}

impl Default for ArchetPatch {
    fn default() -> Self {
        Self {
            name: "Violin".into(),
            instrument: Instrument::Violin,
            auto_range: false,
            pluck: false,
            polyphony: 8,
            friction: FrictionKind::Static,
            bow_pos: 0.13,
            bow_vel: 0.18,
            bow_force: 1.0,
            loss: 0.30,
            slope: 3.0,
            tor_ratio: 5.0,
            tor_couple: 0.10,
            tor_inject: 0.10,
            bridge_hill_db: 9.0,
            bow_noise: 0.13,
            attack: 0.04,
            release: 0.12,
            vib_rate: 5.8,
            vib_depth: 14.0,
            vib_delay: 0.25,
            output_level: 0.9,
            vel_sens: 0.3,
            tune_cents: 0.0,
            seed_offset: 0,
            ensemble: 0.0,
            // The default patch is a soloist, and a soloist is heard in a chamber.
            fx: crate::fx::chain(crate::fx::CHAMBER),
        }
    }
}

impl ArchetPatch {
    pub fn violin() -> Self {
        Self::default()
    }

    /// Violin section: one source widened into an ensemble by the detune
    /// cluster, at constant cost, with a slightly slower attack and a touch
    /// more vibrato, as a desk of players has. Use a low polyphony with
    /// this preset rather than stacking engines.
    pub fn violin_ensemble() -> Self {
        Self {
            name: "Violin Ensemble".into(),
            attack: 0.06,
            release: 0.16,
            vib_depth: 16.0,
            ensemble: 14.0, // 14-player section (Meyer first-desk size)
            polyphony: 24,  // the bounded real-voice pool + note overlaps
            ..Self::default()
        }
    }

    pub fn viola() -> Self {
        Self {
            name: "Viola".into(),
            instrument: Instrument::Viola,
            bridge_hill_db: 7.0,
            bow_pos: 0.12,
            ..Self::default()
        }
    }

    pub fn cello() -> Self {
        Self {
            name: "Cello".into(),
            instrument: Instrument::Cello,
            bridge_hill_db: 6.0,
            bow_pos: 0.10,
            // A low loss keeps the mid harmonics through the bridge: a cello's
            // steady centroid sits near 1.7 kHz.
            loss: 0.16,
            vib_rate: 5.2,
            ..Self::default()
        }
    }


    pub fn double_bass() -> Self {
        Self {
            name: "Double Bass".into(),
            instrument: Instrument::DoubleBass,
            // An arco bass is harmonic-rich: the body radiates the harmonics
            // rather than the 41 Hz fundamental.
            bridge_hill_db: 10.0,  // upper-body presence for the harmonics
            bow_pos: 0.09,         // near the bridge: many harmonics
            loss: 0.22,            // bright: harmonics survive, not a clean sub
            vib_rate: 4.4,
            vib_depth: 7.0,
            bow_noise: 0.14,       // arco bass is noisy
            ..Self::default()
        }
    }

    /// Factory preset bank. Curated, musically-distinct voices the bowed-string /
    /// plucked model can produce across the four instruments and their
    /// articulations (arco, sections, sul tasto/ponticello, pizzicato,
    /// pizzicato). Names are unique (the add-track picker keys on the name;
    /// `factory_preset_names_are_unique` enforces it). A full 16x16 productized
    /// bank lands when Archet ships as a standalone plugin; this is the session-
    /// integration set so a Session/`.phx` track has real starting points.
    pub fn factory_presets() -> Vec<Self> {
        Self::bank()
            .into_iter()
            .map(|mut p| {
                p.fx = crate::fx::chain(crate::fx::space_for(&p));
                p.output_level = PRESET_LEVEL;
                p
            })
            .collect()
    }

    /// The bank before each preset takes the space it is heard in.
    fn bank() -> Vec<Self> {
        let named = |mut p: Self, n: &str| { p.name = n.into(); p };
        vec![
            // -- Solo arco -----------------------------------------------
            named(Self::violin(),      "Violin Solo"),
            named(Self::viola(),       "Viola Solo"),
            named(Self::cello(),       "Cello Solo"),
            named(Self::double_bass(), "Double Bass Solo"),
            // expressive solo characters
            named(Self { vib_depth: 22.0, vib_rate: 6.2, vib_delay: 0.18, attack: 0.05,
                         bridge_hill_db: 10.0, ..Self::violin() }, "Romantic Violin"),
            named(Self { vib_depth: 4.0, vib_rate: 5.0, vib_delay: 0.5, bow_noise: 0.18,
                         bridge_hill_db: 7.0, loss: 0.36, ..Self::violin() }, "Baroque Violin"),
            named(Self { bow_pos: 0.18, loss: 0.5, bow_force: 0.7, bridge_hill_db: 5.0,
                         bow_noise: 0.08, ..Self::violin() }, "Violin Sul Tasto"),
            named(Self { bow_pos: 0.045, loss: 0.1, bow_force: 1.4, bridge_hill_db: 12.0,
                         bow_noise: 0.22, ..Self::violin() }, "Violin Sul Ponticello"),
            named(Self { vib_depth: 18.0, vib_rate: 5.0, attack: 0.07, release: 0.18,
                         ..Self::cello() }, "Lyrical Cello"),
            named(Self { bow_pos: 0.05, loss: 0.12, bridge_hill_db: 12.0, bow_noise: 0.18,
                         ..Self::cello() }, "Cello Ponticello"),
            // -- Sections (ensemble cluster, O(1)) -----------------------
            named(Self::violin_ensemble(), "Violin Section"),
            named(Self { ensemble: 6.0, polyphony: 16, attack: 0.05, ..Self::violin() },
                  "Violin Section Small"),
            named(Self { ensemble: 30.0, polyphony: 32, attack: 0.08, vib_depth: 18.0,
                         ..Self::violin() }, "Violin Section Large"),
            named(Self { ensemble: 12.0, polyphony: 24, attack: 0.06, ..Self::viola() },
                  "Viola Section"),
            named(Self { ensemble: 10.0, polyphony: 24, attack: 0.07, release: 0.18,
                         ..Self::cello() }, "Cello Section"),
            named(Self { ensemble: 8.0, polyphony: 16, attack: 0.08, ..Self::double_bass() },
                  "Bass Section"),
            // full-range composite desk: body picked per note from pitch
            named(Self { auto_range: true, ensemble: 14.0, polyphony: 32, attack: 0.07,
                         release: 0.2, vib_depth: 16.0, name: String::new(),
                         ..Self::default() }, "Full String Ensemble"),
            named(Self { auto_range: true, ensemble: 24.0, polyphony: 40, attack: 0.1,
                         release: 0.25, vib_depth: 18.0, bridge_hill_db: 10.0,
                         ..Self::default() }, "Cinematic Strings"),
            // -- Pizzicato (plucked arco strings) ------------------------
            named(Self { pluck: true, vib_depth: 0.0, bow_noise: 0.0, release: 0.08,
                         ..Self::violin() }, "Violin Pizzicato"),
            named(Self { pluck: true, vib_depth: 0.0, bow_noise: 0.0, release: 0.1,
                         ..Self::cello() }, "Cello Pizzicato"),
            named(Self { pluck: true, vib_depth: 0.0, bow_noise: 0.0, release: 0.12,
                         ..Self::double_bass() }, "Bass Pizzicato"),
            named(Self { pluck: true, vib_depth: 0.0, bow_noise: 0.0, release: 0.06,
                         ensemble: 12.0, polyphony: 32, auto_range: true,
                         ..Self::default() }, "Pizzicato Section"),
        ]
    }
}

/// The output level every factory preset ships at. The engine's own
/// headroom keeps an eight-voice chord at full velocity under full scale
/// with no chain in the way; a preset carries a ceiling, so it sits higher.
pub const PRESET_LEVEL: f32 = 1.4;

impl phonix_plugin::preset::Preset for ArchetPatch {
    fn preset_name(&self) -> &str { &self.name }
    /// Derived section for the picker: composite desk, plucked, or by instrument.
    fn preset_category(&self) -> Option<&str> {
        Some(if self.auto_range {
            "Ensemble"
        } else if self.pluck {
            "Plucked"
        } else {
            match self.instrument {
                Instrument::Violin => "Violin",
                Instrument::Viola => "Viola",
                Instrument::Cello => "Cello",
                Instrument::DoubleBass => "Double Bass",
            }
        })
    }
}

#[cfg(test)]
mod preset_tests {
    use super::*;

    #[test]
    fn factory_preset_names_are_unique() {
        let bank = ArchetPatch::factory_presets();
        assert!(bank.len() >= 16, "bank too small: {}", bank.len());
        let mut seen = std::collections::HashSet::new();
        for p in &bank {
            assert!(!p.name.trim().is_empty(), "empty preset name");
            assert!(seen.insert(p.name.clone()), "duplicate preset name: {}", p.name);
        }
    }

    /// Serde round-trip: a re-serialized patch must be byte-identical, proving no
    /// field is dropped or silently defaulted on .phx save/reload (state loss).
    #[test]
    fn preset_serde_roundtrip() {
        for p in ArchetPatch::factory_presets() {
            let json = serde_json::to_string(&p).unwrap();
            let back: ArchetPatch = serde_json::from_str(&json).unwrap();
            assert_eq!(serde_json::to_string(&back).unwrap(), json,
                "serde round-trip drift for preset '{}'", p.name);
        }
    }
}
