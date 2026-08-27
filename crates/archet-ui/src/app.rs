//! The editor's one composition.

use archet::ArchetPatch;

/// The window takes its size from the constants the composition is laid out
/// against, so the two cannot drift and leave the editor cropped in a host.
pub const W: f32 = 900.0;
pub const H: f32 = 560.0;

pub struct ArchetApp {
    patch: ArchetPatch,
}

impl Default for ArchetApp {
    fn default() -> Self { Self::new() }
}

impl ArchetApp {
    pub fn new() -> Self {
        Self { patch: ArchetPatch::default() }
    }

    /// Mirror the engine into the editor. The editor must never command the
    /// engine on open, or it stomps the state the host just restored.
    pub fn set_patch(&mut self, patch: ArchetPatch) { self.patch = patch; }
    pub fn current_patch(&self) -> ArchetPatch { self.patch.clone() }

    pub fn draw_ui(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Aethon Archet");
            ui.label(&self.patch.name);
            ui.add(egui::Slider::new(&mut self.patch.gain, 0.0..=2.0).text("Gain"));
        });
    }
}
