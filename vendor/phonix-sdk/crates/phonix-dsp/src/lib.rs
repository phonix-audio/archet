//! Shared DSP primitives, none named after an instrument.
//!
//! `samples` is the SampleAsset shape and a procedural library,
//! `sample_player` pitch-tracked looped playback, `granular` a 16-grain
//! scheduler.

pub mod samples;
pub mod sample_player;
pub mod granular;
pub mod fastmath;
pub mod filters;
pub mod lfo;
pub mod ducker;
pub mod vibrato;
pub mod oscillator;
// A 16-bank wavetable oscillator.
pub mod wavetable;
// Distinct from `envelope`: another curve and another retrigger rule.
pub mod adsr;

/// Distinct from `adsr`: the two differ in curve and in retrigger rule and
/// are not interchangeable.
pub mod envelope;
// A FOF choir.
pub mod fof;
// An RMS compressor / limiter.
pub mod rms_compressor;
// The Drum Machine's synthesized rock-kit voices (Canon909 reuses them).
// Engine-agnostic modulation sources, and the ADSR / LFO parameter shapes.
pub mod mod_sources;
pub mod synth_params;
pub mod spectral_resynth;
pub mod pitch;
pub mod reverb;
pub mod chorus;
pub mod meters;
pub mod formant;
pub mod dynamics;
