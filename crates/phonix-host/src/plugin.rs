//! A plugin the sequencer hosts, whichever format it came in: the
//! sequencer speaks to it through one surface.

use std::path::{Path, PathBuf};
use crate::clap::{ClapEditorConn, ClapPlugin};
use crate::vst3::{HostComponentHandler, Vst3EditorConn, Vst3ParamCache, Vst3ParamEntry, Vst3Plugin};

pub enum HostedPlugin {
    Vst3(Vst3Plugin),
    Clap(ClapPlugin),
}

/// Where the transport stands, for a plugin that follows it.
///
/// One shape for both formats, because both want the same facts and the
/// sequencer knows all of them: a tempo-synced delay needs the tempo, an
/// arpeggiator needs the beat, a plugin that draws a playhead needs the bar,
/// and a looper needs to know a cycle is running. The two hosts then say it
/// in their own vocabulary -- a `ProcessContext` for VST3, a
/// `clap_event_transport` for CLAP.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayHead {
    pub tempo: f64,
    pub playing: bool,
    pub recording: bool,
    /// The playhead, in quarter notes from the start of the project.
    pub beats: f64,
    /// Where the bar the playhead is in started, in quarter notes.
    pub bar_start_beats: f64,
    /// The playhead in samples. Derived from the musical position at the
    /// current tempo, so it follows a tempo map only as closely as that.
    pub samples: i64,
    /// Frames rendered since the engine was built. Never jumps, whatever the
    /// playhead does, which is what a plugin smoothing across a locate wants.
    pub continuous_samples: i64,
    pub time_sig: (u8, u8),
    /// The loop, in quarter notes, while one is running.
    pub loop_beats: Option<(f64, f64)>,
    pub sample_rate: f64,
}

impl PlayHead {
    /// The same transport, moved to `beats` quarter notes from the start.
    ///
    /// What a renderer working ahead of the playhead needs: the tempo and the
    /// metre are the block's, but the position is its own. In beats rather
    /// than in a sequencer's ticks, because what a tick is worth is that
    /// sequencer's business.
    pub fn at_beats(&self, beats: f64) -> Self {
        // Bars from the metre the block is in. A denominator of 4 makes a
        // beat a quarter note, which is the unit `beats` is already in.
        let beats_per_bar =
            (self.time_sig.0.max(1) as f64) * 4.0 / (self.time_sig.1.max(1) as f64);
        let bar = (beats / beats_per_bar).floor();
        Self {
            beats,
            bar_start_beats: bar * beats_per_bar,
            samples: (beats * 60.0 / self.tempo.max(1.0) * self.sample_rate) as i64,
            ..*self
        }
    }
}

impl Default for PlayHead {
    fn default() -> Self {
        Self {
            tempo: 120.0,
            playing: false,
            recording: false,
            beats: 0.0,
            bar_start_beats: 0.0,
            samples: 0,
            continuous_samples: 0,
            time_sig: (4, 4),
            loop_beats: None,
            sample_rate: 48_000.0,
        }
    }
}

/// A hosted plugin's own editor, whichever the format.
#[derive(Clone)]
pub enum HostedEditor {
    Vst3(Vst3EditorConn),
    Clap(ClapEditorConn),
}

/// Whether a path names a CLAP plugin rather than a VST3 bundle.
pub fn is_clap_path(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("clap")).unwrap_or(false)
}

/// Load an effect or an instrument by path: the first plugin of a
/// `.clap`, or the VST3 bundle.
pub fn load(path: &Path, sr: f32, block: usize) -> Result<HostedPlugin, String> {
    if is_clap_path(path) {
        ClapPlugin::load(path, None, sr, block).map(HostedPlugin::Clap)
    } else {
        Vst3Plugin::load(path, sr, block).map(HostedPlugin::Vst3)
    }
}

/// Load the instrument a session names: by plugin id in a `.clap`, by
/// class id in a VST3 bundle. Returns the plugin and the path it came from.
pub fn load_for_session(path: &Path, id: Option<&str>, sr: f32, block: usize, offline: bool) -> Result<(HostedPlugin, PathBuf), String> {
    if is_clap_path(path) {
        let id = id.filter(|s| !s.is_empty());
        ClapPlugin::load(path, id, sr, block).map(|p| (HostedPlugin::Clap(p), path.to_path_buf()))
    } else {
        crate::vst3::load_for_session(path, id, sr, block, offline).map(|(p, at)| (HostedPlugin::Vst3(p), at))
    }
}

impl HostedPlugin {
    pub fn has_audio_input(&self) -> bool {
        match self { HostedPlugin::Vst3(p) => p.has_audio_input(), HostedPlugin::Clap(p) => p.has_audio_input() }
    }

    /// Render an instrument's block into interleaved stereo.
    pub fn process(&mut self, out: &mut [f32], frames: usize) {
        match self {
            HostedPlugin::Vst3(p) => p.process(out, frames),
            HostedPlugin::Clap(p) => p.process(None, out, frames),
        }
    }

    /// Run an effect over an interleaved stereo buffer in place.
    pub fn process_effect(&mut self, io: &mut [f32], frames: usize) {
        match self {
            HostedPlugin::Vst3(p) => p.process_effect(io, frames),
            HostedPlugin::Clap(p) => {
                let input: Vec<f32> = io[..frames * 2].to_vec();
                p.process(Some(&input), io, frames);
            }
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        match self { HostedPlugin::Vst3(p) => p.note_on(note, velocity), HostedPlugin::Clap(p) => p.note_on(note, velocity) }
    }

    pub fn note_off(&mut self, note: u8) {
        match self { HostedPlugin::Vst3(p) => p.note_off(note), HostedPlugin::Clap(p) => p.note_off(note) }
    }

    pub fn all_notes_off(&mut self) {
        match self {
            HostedPlugin::Vst3(p) => { for n in 0..=127u8 { p.note_off(n); } }
            HostedPlugin::Clap(p) => p.all_notes_off(),
        }
    }

    /// A control change: the pedal, the wheel, whatever a keyboard sends.
    ///
    /// False means the plugin has no way to take it -- a VST3 that publishes
    /// no mapping for that controller, a CLAP whose note port speaks only the
    /// CLAP dialect -- rather than that anything went wrong.
    pub fn send_cc(&mut self, cc: u8, value: u8, sample_offset: i32) -> bool {
        match self {
            HostedPlugin::Vst3(p) => p.send_cc(cc, value, sample_offset),
            HostedPlugin::Clap(p) => p.send_cc(cc, value),
        }
    }

    /// The pitch wheel, 14 bits, 8192 at rest.
    pub fn pitch_bend(&mut self, value: u16, sample_offset: i32) -> bool {
        match self {
            HostedPlugin::Vst3(p) => p.pitch_bend(value, sample_offset),
            HostedPlugin::Clap(p) => p.pitch_bend(value),
        }
    }

    /// Channel pressure, the aftertouch a keyboard sends for the whole
    /// channel rather than per key.
    pub fn channel_pressure(&mut self, value: u8, sample_offset: i32) -> bool {
        match self {
            HostedPlugin::Vst3(p) => p.channel_pressure(value, sample_offset),
            HostedPlugin::Clap(p) => p.channel_pressure(value),
        }
    }

    /// Set a parameter from a value between 0 and 1.
    pub fn set_param(&mut self, id: u32, value: f64) {
        match self {
            HostedPlugin::Vst3(p) => p.set_param(id, value),
            HostedPlugin::Clap(p) => p.set_param_normalized(id, value as f32),
        }
    }

    pub fn get_state(&self) -> Vec<u8> {
        match self { HostedPlugin::Vst3(p) => p.get_state(), HostedPlugin::Clap(p) => p.save_state() }
    }

    pub fn set_state(&mut self, data: &[u8]) {
        match self { HostedPlugin::Vst3(p) => p.set_state(data), HostedPlugin::Clap(p) => { p.load_state(data); } }
    }

    /// The id the session names the plugin by: the VST3 class id, the
    /// CLAP plugin id.
    pub fn plugin_id(&self) -> String {
        match self { HostedPlugin::Vst3(p) => p.class_id_hex().to_string(), HostedPlugin::Clap(p) => p.info.id.clone() }
    }

    pub fn class_id_hex(&self) -> String { self.plugin_id() }

    pub fn latency_samples(&self) -> usize {
        match self { HostedPlugin::Vst3(p) => p.latency_samples(), HostedPlugin::Clap(p) => p.latency_samples() }
    }

    pub fn refresh_latency(&mut self) {
        match self {
            HostedPlugin::Vst3(p) => p.refresh_latency(),
            HostedPlugin::Clap(p) => { if p.take_params_rescan() { p.refresh_params(); } let _ = p.take_restart(); }
        }
    }

    pub fn editor_conn(&self) -> Option<HostedEditor> {
        match self {
            HostedPlugin::Vst3(p) => p.editor_conn().map(HostedEditor::Vst3),
            HostedPlugin::Clap(p) => p.editor_conn().map(HostedEditor::Clap),
        }
    }

    pub fn set_component_handler(&mut self, handler: Box<HostComponentHandler>) {
        if let HostedPlugin::Vst3(p) = self { p.set_component_handler(handler); }
    }

    /// Whether the plugin changed its own parameters since the last look,
    /// so the interface reads them again.
    pub fn take_param_changed(&self) -> bool {
        match self { HostedPlugin::Vst3(_) => false, HostedPlugin::Clap(p) => p.take_param_changed() || p.take_params_rescan() }
    }

    /// A note expression for every sounding instance of a key, in the
    /// expression's units; VST3 plugins take none.
    pub fn note_expression(&mut self, expression_id: i32, key: u8, value: f64) {
        if let HostedPlugin::Clap(p) = self { p.note_expression(expression_id, key, value); }
    }

    /// Where the transport stands, for plugins that follow it.
    pub fn set_transport(&mut self, head: &PlayHead) {
        match self {
            HostedPlugin::Clap(p) => p.set_transport(head),
            HostedPlugin::Vst3(p) => p.set_transport(head),
        }
    }

    /// The parameters as the generic grid shows them.
    pub fn build_param_cache(&self) -> Vst3ParamCache {
        match self {
            HostedPlugin::Vst3(p) => p.build_param_cache(),
            HostedPlugin::Clap(p) => {
                let params = p.params().iter().map(|info| {
                    let value = p.get_param(info.id).unwrap_or(info.default);
                    Vst3ParamEntry {
                        id: info.id,
                        title: info.name.clone(),
                        short_title: String::new(),
                        units: String::new(),
                        step_count: if info.stepped { ((info.max - info.min).round() as i32).max(1) } else { 0 },
                        value: info.normalize(value) as f64,
                        default: info.normalize(info.default) as f64,
                        flags: 0,
                        display: p.param_text(info.id, value).unwrap_or_default(),
                        enum_labels: Vec::new(),
                    }
                }).collect();
                Vst3ParamCache { params, plugin_name: p.info.name.clone() }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.clap` file loads through the CLAP host, a bundle through VST3.
    #[test]
    fn the_extension_picks_the_host() {
        // Against whatever CLAP effect is installed: this crate ships no
        // plugin, and a test that builds one from a sibling repository is a
        // test that needs a tree beside it.
        let Some(info) = crate::clap::scan_clap().into_iter().find(|p| p.is_effect) else {
            eprintln!("no CLAP effect installed; skipping");
            return;
        };
        let clap = PathBuf::from(&info.path);
        assert!(is_clap_path(&clap));
        let mut p = load(&clap, 48_000.0, 256).unwrap();
        assert!(matches!(p, HostedPlugin::Clap(_)));
        assert!(p.has_audio_input());
        let cache = p.build_param_cache();
        assert!(!cache.params.is_empty());
        assert!(!cache.plugin_name.is_empty());
        // Audio reaches it and comes back finite. NOT that it comes back
        // loud: an effect is entitled to be silent on a constant input, and
        // the first one installed may well be a harmoniser that has heard no
        // pitch yet. What is under test is that the extension chose the right
        // host and the buffers went through it.
        let mut io = vec![0.1f32; 512];
        p.process_effect(&mut io, 256);
        assert!(io.iter().all(|x| x.is_finite()), "the effect returned a NaN");
        let (again, at) = load_for_session(&clap, Some(&p.plugin_id()), 48_000.0, 256, false).unwrap();
        assert_eq!(at, clap);
        assert!(!again.plugin_id().is_empty());
    }
}
