//! The Aethon Archet editor, in egui.
//!
//! A separate crate from the engine, and not a feature of it. Cargo unifies
//! features across a resolved dependency graph, so an optional `egui` inside
//! `archet` would be switched on for the engine's own tests by any
//! `cargo test --workspace`. A crate boundary is the only thing that makes
//! "the engine never sees egui" true rather than merely intended.

pub mod app;

pub use app::ArchetApp;
