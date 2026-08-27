//! Aethon Archet — the engine.
//!
//! TODO(extraction): move the DSP here from aethon/src/... . The engine's
//! contract is that it never sees egui and never sees a plugin framework: it
//! takes a sample rate, a patch and MIDI, and fills a buffer.

use serde::{Deserialize, Serialize};

/// Everything the editor edits and the host persists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchetPatch {
    pub name: String,
    pub gain: f32,
}

impl Default for ArchetPatch {
    fn default() -> Self {
        Self { name: "Init".to_string(), gain: 0.9 }
    }
}

/// The factory bank. Order is a compatibility surface: a host stores a preset
/// as an index. See COMPAT.md.
pub fn factory_presets() -> Vec<ArchetPatch> {
    vec![ArchetPatch::default()]
}

pub struct ArchetEngine {
    sample_rate: f32,
    patch: ArchetPatch,
}

impl ArchetEngine {
    pub fn new(sample_rate: f32) -> Self {
        Self { sample_rate, patch: ArchetPatch::default() }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
    }

    pub fn sample_rate(&self) -> f32 { self.sample_rate }

    pub fn set_patch(&mut self, patch: ArchetPatch) { self.patch = patch; }
    pub fn patch(&self) -> &ArchetPatch { &self.patch }

    pub fn note_on(&mut self, _note: u8, _velocity: u8) {}
    pub fn note_off(&mut self, _note: u8) {}
    pub fn all_notes_off(&mut self) {}

    /// Interleaved, `channels` wide. Additive into `out`, like every other
    /// engine in the family.
    pub fn process_audio(&mut self, _out: &mut [f32], _channels: usize) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_patch_round_trips_through_serde() {
        let p = ArchetPatch::default();
        let s = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<ArchetPatch>(&s).unwrap(), p);
    }
}
