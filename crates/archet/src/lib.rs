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
pub mod harpsichord;
pub mod modal;
pub mod patch;
pub mod section;
pub mod string;
pub mod sympathetic;
pub mod voice;

pub use engine::{ArchetCommand, ArchetEngine, ArchetMeterState};
pub use patch::ArchetPatch;

// ── The shared crates, under the module names the engine source uses ───────
//
// The engine came out of the monolith reaching `crate::dsp::filters` and
// `crate::sequencer::state_buffer`. Re-exporting the shared crates under those
// names is what lets the DSP move byte-for-byte: two lines here instead of a
// sed over every call site, which is also what makes "the golden hash is
// unchanged" a claim rather than a hope.
//
// NEVER copy shared DSP in here to make the crate look self-contained. The
// only thing Archet takes from `phonix-dsp` is `filters::BiquadT` (the modal
// body's biquad); the bowed-string physical model itself — the friction
// junction, the waveguide, the modal body, the sympathetic strings, the
// harpsichord soundboard and the section diffuser — has no counterpart in the
// SDK and moved here untouched. `grep -ril 'karplus\|waveguide\|bowed\|friction'`
// over `crates/` finds nothing of the kind in `phonix-dsp`.
pub use phonix_dsp as dsp;
pub use phonix_rt as state_buffer;
