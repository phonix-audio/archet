//! Aethon Archet — VST3 / CLAP, via nice-plug.

use nice_plug::prelude::*;
use nice_plug_egui::{create_egui_editor, EguiState};
use std::sync::{Arc, RwLock};

use archet::{ArchetEngine, ArchetPatch};
use archet_ui::ArchetApp;

pub struct ArchetPlugin {
    params: Arc<ArchetParams>,
    engine: Option<ArchetEngine>,
    buf: Vec<f32>,
}

impl Default for ArchetPlugin {
    fn default() -> Self {
        Self { params: Arc::new(ArchetParams::default()), engine: None, buf: Vec::new() }
    }
}

#[derive(Params)]
struct ArchetParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,
    #[persist = "patch"]
    patch_state: Arc<RwLock<ArchetPatch>>,
    #[id = "gain"] gain: FloatParam,
}

impl Default for ArchetParams {
    fn default() -> Self {
        Self {
            editor_state: EguiState::from_size(archet_ui::app::W as u32, archet_ui::app::H as u32),
            patch_state: Arc::new(RwLock::new(ArchetPatch::default())),
            gain: FloatParam::new("Gain", 0.9, FloatRange::Linear { min: 0.0, max: 2.0 }),
        }
    }
}

impl Plugin for ArchetPlugin {
    const NAME: &'static str = "Aethon Archet";
    const VENDOR: &'static str = "Aethon Audio";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: None,
        main_output_channels: Some(unsafe { std::num::NonZeroU32::new_unchecked(2) }),
        aux_input_ports: &[], aux_output_ports: &[], names: PortNames::const_default(),
    }];
    // MidiCCs, not Basic: nice-plug only publishes the IMidiMapping a VST3 host
    // needs when this is MidiCCs. With Basic, a NoteEvent::MidiCC arm is dead
    // code in every VST3 host, so a pedal or a mod wheel never arrives at all.
    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> { self.params.clone() }

    fn editor(&mut self, _ax: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let patch_state = self.params.patch_state.clone();
        let mut app = ArchetApp::new();
        // Seed the editor from the restored state before its first frame, or
        // the closure below publishes the default patch over it.
        if let Ok(p) = patch_state.read() { app.set_patch(p.clone()); }
        create_egui_editor(
            self.params.editor_state.clone(), app, Default::default(),
            |_c, _q, _a| {},
            move |ui, _setter, _q, app| {
                let ctx = ui.ctx().clone();
                app.draw_ui(&ctx);
                if let Ok(mut p) = patch_state.write() { *p = app.current_patch(); }
            },
        )
    }

    /// Must be re-entrant. nice-plug calls this again from `set_state` whenever
    /// a buffer config already exists, and a host that calls
    /// `setup_processing` before restoring state always takes the second path.
    fn initialize(&mut self, _l: &AudioIOLayout, cfg: &BufferConfig, _c: &mut impl InitContext<Self>) -> bool {
        match self.engine.as_mut() {
            Some(e) => e.set_sample_rate(cfg.sample_rate),
            None => self.engine = Some(ArchetEngine::new(cfg.sample_rate)),
        }
        self.buf = vec![0.0; cfg.max_buffer_size as usize * 2];
        let patch = self.params.patch_state.read().map(|p| p.clone()).unwrap_or_default();
        if let Some(e) = self.engine.as_mut() { e.set_patch(patch); }
        true
    }

    fn reset(&mut self) {}

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, ctx: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let engine = match self.engine.as_mut() { Some(e) => e, None => return ProcessStatus::Normal };

        while let Some(ev) = ctx.next_event() {
            match ev {
                NoteEvent::NoteOn { note, velocity, .. } => engine.note_on(note, (velocity * 127.0) as u8),
                NoteEvent::NoteOff { note, .. } => engine.note_off(note),
                _ => {}
            }
        }

        let n = buffer.samples();
        let il = n * 2;
        if self.buf.len() < il { self.buf.resize(il, 0.0); }
        for s in &mut self.buf[..il] { *s = 0.0; }
        engine.process_audio(&mut self.buf[..il], 2);
        let ch = buffer.as_slice();
        if ch.len() >= 2 {
            let (l, r) = ch.split_at_mut(1);
            for i in 0..n { l[0][i] = self.buf[i * 2]; r[0][i] = self.buf[i * 2 + 1]; }
        } else if !ch.is_empty() {
            for i in 0..n { ch[0][i] = (self.buf[i * 2] + self.buf[i * 2 + 1]) * 0.5; }
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for ArchetPlugin {
    const CLAP_ID: &'static str = "com.aethon-audio.archet";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Aethon Archet");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Stereo];
}

impl Vst3Plugin for ArchetPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"VxArchetBow00001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

nice_export_clap!(ArchetPlugin);
nice_export_vst3!(ArchetPlugin);

// ── Frozen identifiers ────────────────────────────────────────────
//
// These four strings are a compatibility surface, not a naming choice. The
// class id resolves an exported DAWproject `Vst3Plugin` device and sits in the
// header of every `.vstpreset`; the CLAP id identifies the plugin to a CLAP
// host; NAME and VENDOR compose the directory Cubase's MediaBay indexes.
// Change any of them and existing projects and preset banks point at a plugin
// no host can find. See COMPAT.md.
#[cfg(test)]
mod frozen_identifiers {
    use super::*;

    #[test]
    fn the_ids_a_host_resolves_us_by_have_not_moved() {
        assert_eq!(<ArchetPlugin as Vst3Plugin>::VST3_CLASS_ID, *b"VxArchetBow00001");
        assert_eq!(<ArchetPlugin as ClapPlugin>::CLAP_ID, "com.aethon-audio.archet");
        assert_eq!(<ArchetPlugin as Plugin>::NAME, "Aethon Archet");
        assert_eq!(<ArchetPlugin as Plugin>::VENDOR, "Aethon Audio");
    }
}
