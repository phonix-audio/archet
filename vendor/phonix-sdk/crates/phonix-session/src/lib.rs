//! Host-agnostic session data model.
//!
//! The serializable shape of a session — `Session`, `TrackConfig`, `Pattern`,
//! `MidiNote`, the modulation matrix, the groove template and the scale
//! tables — hoisted out of the sequencer so tools that only read or write
//! sessions (importers, exporters, analysers) need none of the audio engine,
//! the effects rack or the GUI.
//!
//! Everything here is pure data plus the operations that need nothing but the
//! data. Anything that touches a mixer, an effects chain or a widget stays in
//! the host and is written against these types.

pub mod data;
pub mod tempo;
pub mod surround;
pub mod mod_matrix;
// The shared arpeggiator: pattern, clock, scale, velocity and step logic.

pub use phonix_fx::ReverbKind;
