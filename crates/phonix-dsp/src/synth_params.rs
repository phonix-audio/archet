//! Engine-agnostic synth parameter shapes.
//!
//! `EnvParams` (an ADSR quadruple with a few named presets) and `LfoParams`
//! (waveform index, rate, depth, key/tempo sync) describe nothing
//! engine-specific, so they live here and are shared by the engines that
//! serialize them. Each engine keeps its own patch struct around them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvParams {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for EnvParams {
    fn default() -> Self {
        Self { attack: 0.01, decay: 0.3, sustain: 0.7, release: 0.4 }
    }
}

impl EnvParams {
    pub fn fast() -> Self {
        Self { attack: 0.001, decay: 0.1, sustain: 0.0, release: 0.05 }
    }
    pub fn pluck() -> Self {
        Self { attack: 0.001, decay: 0.5, sustain: 0.0, release: 0.2 }
    }
    pub fn pad() -> Self {
        Self { attack: 0.6, decay: 0.4, sustain: 0.8, release: 1.2 }
    }
    pub fn organ() -> Self {
        Self { attack: 0.01, decay: 0.0, sustain: 1.0, release: 0.05 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LfoParams {
    /// Waveform index, in the engine's own shape table
    pub waveform: usize,
    /// Rate in Hz (0.01..20.0)
    pub rate: f32,
    /// Depth 0..1 (overall output scale)
    pub depth: f32,
    pub key_sync: bool,
    pub tempo_sync: bool,
    /// Index into TEMPO_DIVISIONS table when tempo_sync=true
    pub tempo_division: usize,
}

impl Default for LfoParams {
    fn default() -> Self {
        Self {
            waveform: 0, // Sine
            rate: 2.0,
            depth: 0.5,
            key_sync: false,
            tempo_sync: false,
            tempo_division: 4, // 1/4 note
        }
    }
}
