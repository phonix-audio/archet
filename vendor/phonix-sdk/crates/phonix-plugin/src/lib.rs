//! Phonix's preset layer.
//!
//! The contract a factory bank exposes (`preset`) and the writer that publishes
//! a bank as `.vstpreset` files a host can index (`vstpreset`). Every engine's
//! patch type implements `Preset`, and `generate_patch_presets` turns a bank of
//! them into a MediaBay-browsable tree — so the two belong together, one layer
//! below every plugin.
//!
//! No egui: the preset-picker WIDGET that draws a bank lives in the
//! `phonix-ui` crate and depends on this one for the trait, not the reverse.

// The insert-FX chain a preset may describe lives in the effects crate;
// re-exported so `crate::fx_chain::FxChainSpec` resolves exactly as it did in
// the application.

pub mod preset;
pub mod vstpreset;
// Which in-house engines ship as a VST3 plugin, and the class ids the
// `.vstpreset` writer and the DAWproject exporter both publish them under.
pub mod identity;
pub mod instrument;
