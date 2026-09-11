//! Archet — bowed-strings physical-model engine (violin / viola / cello / bass).
//!
//! A dedicated digital-waveguide bowed string with a procedural modal *body*
//! filter — the element that turns a bare string (spectrally an organ/reed) into
//! a recognizable violin. Built from the acoustics literature (CCRMA / IRCAM /
//! Woodhouse); see the per-module docs and the plan's Sources for citations.
//!
//! Signal flow per voice:
//!   bow -> [friction junction] -> transverse + torsional waveguide -> bridge
//!   force -> [modal body filter] -> radiated sound.

pub mod body;
pub mod engine;
pub mod friction;
pub mod modal;
pub mod patch;
pub mod section;
pub mod string;
pub mod sympathetic;
pub mod voice;

pub use engine::{ArchetCommand, ArchetEngine, ArchetMeterState};
pub use patch::ArchetPatch;
