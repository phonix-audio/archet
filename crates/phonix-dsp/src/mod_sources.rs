//! Engine-agnostic modulation SOURCES.
//!
//! `ModSource` and its per-sample `ModSourceValues` snapshot (3 LFOs, four
//! global envelopes, four operator envelopes, velocity, key-track, pitch-bend,
//! mod-wheel, aftertouch and 8 macros) carry nothing engine-specific, so they
//! live here and are shared. Every engine keeps its OWN destination set and
//! its own slot/matrix types.

use serde::{Deserialize, Serialize};

// ─── Sources ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModSource {
    Lfo1,
    Lfo2,
    Lfo3,
    EnvAmp,
    EnvFilter,
    EnvMod,
    EnvPitch,
    OpEnv1,
    OpEnv2,
    OpEnv3,
    OpEnv4,
    Velocity,
    KeyTrack,
    PitchBend,
    ModWheel,
    AfterTouch,
    Macro1,
    Macro2,
    Macro3,
    Macro4,
    Macro5,
    Macro6,
    Macro7,
    Macro8,
}

impl ModSource {
    pub const ALL: &'static [ModSource] = &[
        ModSource::Lfo1,
        ModSource::Lfo2,
        ModSource::Lfo3,
        ModSource::EnvAmp,
        ModSource::EnvFilter,
        ModSource::EnvMod,
        ModSource::EnvPitch,
        ModSource::OpEnv1,
        ModSource::OpEnv2,
        ModSource::OpEnv3,
        ModSource::OpEnv4,
        ModSource::Velocity,
        ModSource::KeyTrack,
        ModSource::PitchBend,
        ModSource::ModWheel,
        ModSource::AfterTouch,
        ModSource::Macro1,
        ModSource::Macro2,
        ModSource::Macro3,
        ModSource::Macro4,
        ModSource::Macro5,
        ModSource::Macro6,
        ModSource::Macro7,
        ModSource::Macro8,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            ModSource::Lfo1 => "LFO 1",
            ModSource::Lfo2 => "LFO 2",
            ModSource::Lfo3 => "LFO 3",
            ModSource::EnvAmp => "Amp Env",
            ModSource::EnvFilter => "Filter Env",
            ModSource::EnvMod => "Mod Env",
            ModSource::EnvPitch => "Pitch Env",
            ModSource::OpEnv1 => "Op 1 Env",
            ModSource::OpEnv2 => "Op 2 Env",
            ModSource::OpEnv3 => "Op 3 Env",
            ModSource::OpEnv4 => "Op 4 Env",
            ModSource::Velocity => "Velocity",
            ModSource::KeyTrack => "Key Track",
            ModSource::PitchBend => "Pitch Bend",
            ModSource::ModWheel => "Mod Wheel",
            ModSource::AfterTouch => "Aftertouch",
            ModSource::Macro1 => "Macro 1",
            ModSource::Macro2 => "Macro 2",
            ModSource::Macro3 => "Macro 3",
            ModSource::Macro4 => "Macro 4",
            ModSource::Macro5 => "Macro 5",
            ModSource::Macro6 => "Macro 6",
            ModSource::Macro7 => "Macro 7",
            ModSource::Macro8 => "Macro 8",
        }
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(ModSource::Lfo1)
    }

    pub fn index(&self) -> usize {
        Self::ALL.iter().position(|s| s == self).unwrap_or(0)
    }
}

/// Snapshot of all source values for one audio sample
#[derive(Debug, Clone, Default)]
pub struct ModSourceValues {
    pub lfo: [f32; 3],
    pub env_amp: f32,
    pub env_filter: f32,
    pub env_mod: f32,
    pub env_pitch: f32,
    pub op_envs: [f32; 4],
    pub velocity: f32,
    pub key_track: f32,   // (note - 60) / 60, range -1..+1
    pub pitch_bend: f32,  // semitones, range -24..+24
    pub mod_wheel: f32,   // 0..1
    pub aftertouch: f32,  // 0..1
    pub macros: [f32; 8], // 0..1
}

impl ModSourceValues {
    pub fn get(&self, src: ModSource) -> f32 {
        match src {
            ModSource::Lfo1 => self.lfo[0],
            ModSource::Lfo2 => self.lfo[1],
            ModSource::Lfo3 => self.lfo[2],
            ModSource::EnvAmp => self.env_amp,
            ModSource::EnvFilter => self.env_filter,
            ModSource::EnvMod => self.env_mod,
            ModSource::EnvPitch => self.env_pitch,
            ModSource::OpEnv1 => self.op_envs[0],
            ModSource::OpEnv2 => self.op_envs[1],
            ModSource::OpEnv3 => self.op_envs[2],
            ModSource::OpEnv4 => self.op_envs[3],
            ModSource::Velocity => self.velocity,
            ModSource::KeyTrack => self.key_track,
            ModSource::PitchBend => self.pitch_bend / 24.0,
            ModSource::ModWheel => self.mod_wheel,
            ModSource::AfterTouch => self.aftertouch,
            ModSource::Macro1 => self.macros[0],
            ModSource::Macro2 => self.macros[1],
            ModSource::Macro3 => self.macros[2],
            ModSource::Macro4 => self.macros[3],
            ModSource::Macro5 => self.macros[4],
            ModSource::Macro6 => self.macros[5],
            ModSource::Macro7 => self.macros[6],
            ModSource::Macro8 => self.macros[7],
        }
    }
}
