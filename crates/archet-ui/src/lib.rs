//! The Archet editor, in egui.
//!
//! **The instrument is the interface.** Archet is a bow on a string driving
//! a body, so the panel is that, drawn: the string from nut to bridge across
//! the top with the bow crossing it where the engine is bowing, leaning as
//! the force comes up; the corpus in the middle, its response the envelope
//! the engine builds its mode bank against, with the four bodies in a rail
//! beside it; the player at the bottom, holding the attack, the vibrato and
//! the size of the section.
//!
//! A separate crate from the engine, and not a feature of it. Cargo unifies
//! features across a resolved dependency graph, so an optional `egui` inside
//! `archet` would be switched on for the engine's own tests by any
//! `cargo test --workspace`. A crate boundary is the only thing that makes
//! "the engine never sees egui" true rather than merely intended.
//!
//! The shared layer (`phonix-ui`) still owns everything that is not specific
//! to this instrument: the chrome, the preset picker and its disk dialogs,
//! the knob body, the keyboard and the mirror-adopt gate. Only the machine
//! itself is drawn here.

pub mod app;
pub mod colors;
pub mod machine;
pub mod panel_geom;
pub mod theme;

pub use app::ArchetApp;

/// Where a saved patch goes on disk.
pub const PRESET_HOME: phonix_ui::preset_io::PresetHome =
    phonix_ui::preset_io::PresetHome { organisation: "Phonix Audio", application: "Archet" };
