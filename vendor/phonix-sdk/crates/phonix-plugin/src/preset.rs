//! The contract a preset bank exposes.
//!
//! `Preset` is what every engine's patch type implements so the rest of the
//! application can read a bank without knowing the engine: the shared picker
//! widget draws it, and the `.vstpreset` writer names and files it. It carries
//! no egui — the drawing half is `phonix_ui::preset_picker`.

/// Minimal contract a preset has to expose for the picker.
pub trait Preset {
    fn preset_name(&self) -> &str;
    /// Optional category tag. When the bank has 2+ distinct
    /// categories, the picker auto-enables section headers and
    /// category-scoped prev/next.
    fn preset_category(&self) -> Option<&str> { None }
}

/// Lightweight adapter for engines whose preset list is just
/// `(name, category)` pairs (voice presets, drum
/// patterns, drum kits). Construct on the fly from any source and
/// hand to `picker_ui` without having to invent a wrapper type.
pub struct NamedPreset<'a> {
    pub name:     &'a str,
    pub category: Option<&'a str>,
}

impl<'a> Preset for NamedPreset<'a> {
    fn preset_name(&self) -> &str { self.name }
    fn preset_category(&self) -> Option<&str> { self.category }
}
