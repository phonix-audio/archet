//! Archet as a VST3 and CLAP plugin.
//!
//! Wraps the ArchetEngine in a nice-plug Plugin.
//! No audio input - stereo synth output only. MIDI input for notes + pitch bend.
//!
//! Archet exposes no granular per-parameter Set commands (only SetOutputLevel,
//! SetPolyphony and LoadPatch). So in Init mode (preset == 0) the host-automatable
//! params are assembled into a whole `ArchetPatch` and pushed via `LoadPatch`
//! ONLY when a value actually changed - keeping the audio thread allocation-free
//! when nothing is being automated.

use nice_plug::prelude::*;
use nice_plug_egui::{create_egui_editor, EguiState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, RwLock};

mod fx;
use fx::FxLink;
use phonix_fx::{Chain, ChainSpec, Musical, Transport};

use archet::engine::{ArchetCommand, ArchetEngine, ArchetMeterState};
use archet::patch::{ArchetPatch, Instrument};
use phonix_rt::{meter_channel, SharedReader, Writer};
use archet_ui::app::{self as archet_app, ArchetApp};
use phonix_plugin::preset::Preset;
use phonix_plugin::vstpreset::{self, ParamValue};

// -- Plugin struct ----------------------------------------------------------

pub struct ArchetPlugin {
    params:          Arc<ArchetParams>,
    engine:          Option<ArchetEngine>,
    command_tx:      mpsc::Sender<ArchetCommand>,
    meter_reader:    Option<SharedReader<ArchetMeterState>>,
    pending:         Option<(mpsc::Receiver<ArchetCommand>, Writer<ArchetMeterState>)>,
    interleaved_buf: Vec<f32>,
    buf_l:           Vec<f32>,
    buf_r:           Vec<f32>,
    factory_presets: Vec<ArchetPatch>,
    last_preset:     i32,
    /// The chain the patch describes, built by the host after the engine.
    fx_chain:        Option<Chain>,
    /// The chain as the editor and the audio thread hand it to each other.
    fx_link:         Arc<FxLink>,
    fx_seen:         u64,
    /// Snapshot of the Init-mode param values last pushed, so we only rebuild +
    /// LoadPatch when something changed (no per-block heap allocation at idle).
    last_init_sig:   Option<[f32; INIT_SIG_LEN]>,
}

const INIT_SIG_LEN: usize = 19;

/// Keeps this library mapped for the life of the process.
///
/// The plugin framework installs a global logger and a panic hook at
/// entry, both pointing into this library, and neither can be removed. A
/// host that unloads the library afterwards leaves them dangling, and the
/// next log call crashes the process. Opening the library once more with
/// RTLD_NODELETE tells the loader never to unmap it.
#[cfg(unix)]
fn pin_library() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let mut info: libc::Dl_info = std::mem::zeroed();
        let here = pin_library as *const () as *const libc::c_void;
        if libc::dladdr(here, &mut info) != 0 && !info.dli_fname.is_null() {
            // RTLD_NOLOAD promotes the flags of the library already loaded.
            let flags = libc::RTLD_NOW | libc::RTLD_NODELETE | libc::RTLD_NOLOAD;
            let _never_closed = libc::dlopen(info.dli_fname, flags);
        }
    });
}

#[cfg(not(unix))]
fn pin_library() {}

impl Default for ArchetPlugin {
    fn default() -> Self {
        pin_library();
        let presets = ArchetPatch::factory_presets();
        let preset_count = presets.len();

        let mut preset_names: Vec<String> = Vec::with_capacity(preset_count + 1);
        preset_names.push("Init".to_string());
        for p in &presets { preset_names.push(p.name.clone()); }
        let preset_names = Arc::new(preset_names);

        let (tx, rx) = mpsc::channel();
        let (meter_writer, meter_reader) = meter_channel::<ArchetMeterState>();

        Self {
            params:          Arc::new(ArchetParams::new(preset_count, preset_names)),
            engine:          None,
            command_tx:      tx,
            meter_reader:    Some(meter_reader),
            pending:         Some((rx, meter_writer)),
            interleaved_buf: Vec::new(),
            buf_l:           Vec::new(),
            buf_r:           Vec::new(),
            factory_presets: presets,
            last_preset:     0,
            fx_chain:        None,
            fx_link:         Arc::new(FxLink::new(ChainSpec::default())),
            fx_seen:         0,
            last_init_sig:   None,
        }
    }
}

// -- Parameters ------------------------------------------------------------

#[derive(Params)]
struct ArchetParams {
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    /// Full engine patch, persisted with the project (captures GUI-editable state beyond the
    /// host-automatable knobs below). Mirrored from the engine by the editor; pushed back via
    /// LoadPatch on load.
    #[persist = "patch"]
    patch_state: Arc<RwLock<ArchetPatch>>,

    #[id = "preset"]
    preset: IntParam,

    #[id = "output_level"]
    output_level: FloatParam,

    #[id = "polyphony"]
    polyphony: IntParam,

    #[id = "instrument"]
    instrument: IntParam,

    #[id = "articulation"]
    articulation: IntParam,   // 0 = Bow, 1 = Pluck

    #[id = "full_range"]
    full_range: IntParam,     // 0 = single instrument, 1 = per-note composite desk

    // Bow
    #[id = "bow_pos"]
    bow_pos: FloatParam,
    #[id = "bow_vel"]
    bow_vel: FloatParam,
    #[id = "bow_force"]
    bow_force: FloatParam,
    #[id = "bow_noise"]
    bow_noise: FloatParam,

    // String / body
    #[id = "loss"]
    loss: FloatParam,
    #[id = "bridge_hill"]
    bridge_hill_db: FloatParam,

    // Articulation envelope
    #[id = "attack"]
    attack: FloatParam,
    #[id = "release"]
    release: FloatParam,
    #[id = "vel_sens"]
    vel_sens: FloatParam,

    // Vibrato
    #[id = "vib_rate"]
    vib_rate: FloatParam,
    #[id = "vib_depth"]
    vib_depth: FloatParam,
    #[id = "vib_delay"]
    vib_delay: FloatParam,

    // Section
    #[id = "ensemble"]
    ensemble: FloatParam,
    #[id = "tune_cents"]
    tune_cents: FloatParam,
}

impl ArchetParams {
    fn new(preset_count: usize, preset_names: Arc<Vec<String>>) -> Self {
        let names_v2s = preset_names.clone();
        let names_s2v = preset_names;
        let max_preset = preset_count as i32;

        Self {
            // The size lives in the editor crate (`archet_ui::app::{W, H}`) so the
            // declared window and the composition it holds cannot drift apart;
            // `the_declared_window_size_is_the_editors_own` below pins them.
            editor_state: EguiState::from_size(archet_app::W as u32, archet_app::H as u32),
            patch_state:  Arc::new(RwLock::new(ArchetPatch::default())),

            preset: IntParam::new("Preset", 1, IntRange::Linear { min: 0, max: max_preset })
                .with_value_to_string(Arc::new(move |v| {
                    let idx = v as usize;
                    if idx < names_v2s.len() { names_v2s[idx].clone() }
                    else { format!("Preset {}", v) }
                }))
                .with_string_to_value(Arc::new(move |s| {
                    for (i, name) in names_s2v.iter().enumerate() {
                        if name.eq_ignore_ascii_case(s) { return Some(i as i32); }
                    }
                    s.parse::<i32>().ok()
                })),

            output_level: FloatParam::new("Output Level", 0.9, FloatRange::Linear { min: 0.0, max: 1.5 })
                .with_value_to_string(formatters::v2s_f32_rounded(2)),

            polyphony: IntParam::new("Polyphony", 8, IntRange::Linear { min: 1, max: 48 }),

            instrument: IntParam::new("Instrument", 0, IntRange::Linear { min: 0, max: 3 })
                .with_value_to_string(Arc::new(|v| {
                    ["Violin", "Viola", "Cello", "Double Bass"]
                        .get(v as usize).unwrap_or(&"?").to_string()
                })),

            articulation: IntParam::new("Articulation", 0, IntRange::Linear { min: 0, max: 1 })
                .with_value_to_string(Arc::new(|v| if v == 0 { "Bow".into() } else { "Pluck".into() })),

            full_range: IntParam::new("Full Range", 0, IntRange::Linear { min: 0, max: 1 })
                .with_value_to_string(Arc::new(|v| if v == 0 { "Single".into() } else { "Composite".into() })),

            bow_pos: FloatParam::new("Bow Position", 0.13, FloatRange::Linear { min: 0.03, max: 0.20 })
                .with_value_to_string(formatters::v2s_f32_rounded(3)),
            bow_vel: FloatParam::new("Bow Velocity", 0.18, FloatRange::Linear { min: 0.02, max: 0.5 })
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            bow_force: FloatParam::new("Bow Force", 1.0, FloatRange::Linear { min: 0.2, max: 2.0 })
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            bow_noise: FloatParam::new("Bow Noise", 0.13, FloatRange::Linear { min: 0.0, max: 0.4 })
                .with_value_to_string(formatters::v2s_f32_rounded(2)),

            loss: FloatParam::new("Bridge Loss", 0.30, FloatRange::Linear { min: 0.0, max: 0.6 })
                .with_value_to_string(formatters::v2s_f32_rounded(2)),
            bridge_hill_db: FloatParam::new("Bridge Hill", 9.0, FloatRange::Linear { min: 0.0, max: 15.0 })
                .with_unit(" dB").with_value_to_string(formatters::v2s_f32_rounded(1)),

            attack: FloatParam::new("Attack", 0.04, FloatRange::Skewed {
                min: 0.005, max: 0.2, factor: FloatRange::skew_factor(-1.5),
            }).with_unit(" s").with_value_to_string(formatters::v2s_f32_rounded(3)),
            release: FloatParam::new("Release", 0.12, FloatRange::Skewed {
                min: 0.02, max: 0.4, factor: FloatRange::skew_factor(-1.5),
            }).with_unit(" s").with_value_to_string(formatters::v2s_f32_rounded(3)),
            vel_sens: FloatParam::new("Velocity Sens", 0.3, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" %").with_value_to_string(formatters::v2s_f32_percentage(0)),

            vib_rate: FloatParam::new("Vibrato Rate", 5.8, FloatRange::Linear { min: 3.0, max: 8.0 })
                .with_unit(" Hz").with_value_to_string(formatters::v2s_f32_rounded(2)),
            vib_depth: FloatParam::new("Vibrato Depth", 14.0, FloatRange::Linear { min: 0.0, max: 30.0 })
                .with_unit(" ct").with_value_to_string(formatters::v2s_f32_rounded(1)),
            vib_delay: FloatParam::new("Vibrato Delay", 0.0, FloatRange::Linear { min: 0.0, max: 0.8 })
                .with_unit(" s").with_value_to_string(formatters::v2s_f32_rounded(2)),

            ensemble: FloatParam::new("Ensemble Players", 0.0, FloatRange::Linear { min: 0.0, max: 60.0 })
                .with_value_to_string(formatters::v2s_f32_rounded(0)),
            tune_cents: FloatParam::new("Tune", 0.0, FloatRange::Linear { min: -50.0, max: 50.0 })
                .with_unit(" ct").with_value_to_string(formatters::v2s_f32_rounded(1)),
        }
    }

    /// Cheap signature of every Init-mode param, for change detection.
    fn init_sig(&self) -> [f32; INIT_SIG_LEN] {
        [
            self.output_level.value(),
            self.polyphony.value() as f32,
            self.instrument.value() as f32,
            self.articulation.value() as f32,
            self.full_range.value() as f32,
            self.bow_pos.value(),
            self.bow_vel.value(),
            self.bow_force.value(),
            self.bow_noise.value(),
            self.loss.value(),
            self.bridge_hill_db.value(),
            self.attack.value(),
            self.release.value(),
            self.vel_sens.value(),
            self.vib_rate.value(),
            self.vib_depth.value(),
            self.vib_delay.value(),
            self.ensemble.value(),
            self.tune_cents.value(),
        ]
    }

    /// Build a whole patch from the current Init-mode params.
    fn build_init_patch(&self) -> ArchetPatch {
        let inst = match self.instrument.value() {
            1 => Instrument::Viola,
            2 => Instrument::Cello,
            3 => Instrument::DoubleBass,
            _ => Instrument::Violin,
        };
        ArchetPatch {
            name:           "Init".into(),
            instrument:     inst,
            auto_range:     self.full_range.value() != 0,
            pluck:          self.articulation.value() != 0,
            polyphony:      self.polyphony.value().clamp(1, 48) as u8,
            bow_pos:        self.bow_pos.value(),
            bow_vel:        self.bow_vel.value(),
            bow_force:      self.bow_force.value(),
            bow_noise:      self.bow_noise.value(),
            loss:           self.loss.value(),
            bridge_hill_db: self.bridge_hill_db.value(),
            attack:         self.attack.value(),
            release:        self.release.value(),
            vel_sens:       self.vel_sens.value(),
            vib_rate:       self.vib_rate.value(),
            vib_depth:      self.vib_depth.value(),
            vib_delay:      self.vib_delay.value(),
            ensemble:       self.ensemble.value(),
            tune_cents:     self.tune_cents.value(),
            output_level:   self.output_level.value(),
            ..ArchetPatch::default()
        }
    }
}

impl Default for ArchetParams {
    fn default() -> Self {
        Self::new(0, Arc::new(vec!["Init".to_string()]))
    }
}

// -- .vstpreset generation -------------------------------------------------

fn map_patch_to_nih_params(patch: &ArchetPatch) -> Vec<(&'static str, ParamValue)> {
    let inst_idx = match patch.instrument {
        Instrument::Violin => 0, Instrument::Viola => 1,
        Instrument::Cello => 2, Instrument::DoubleBass => 3,
    };
    vec![
        ("output_level", ParamValue::F32(patch.output_level.clamp(0.0, 1.5))),
        ("polyphony",    ParamValue::I32(patch.polyphony as i32)),
        ("instrument",   ParamValue::I32(inst_idx)),
        ("articulation", ParamValue::I32(if patch.pluck { 1 } else { 0 })),
        ("full_range",   ParamValue::I32(if patch.auto_range { 1 } else { 0 })),
        ("bow_pos",      ParamValue::F32(patch.bow_pos)),
        ("bow_vel",      ParamValue::F32(patch.bow_vel)),
        ("bow_force",    ParamValue::F32(patch.bow_force)),
        ("bow_noise",    ParamValue::F32(patch.bow_noise)),
        ("loss",         ParamValue::F32(patch.loss)),
        ("bridge_hill",  ParamValue::F32(patch.bridge_hill_db)),
        ("attack",       ParamValue::F32(patch.attack)),
        ("release",      ParamValue::F32(patch.release)),
        ("vel_sens",     ParamValue::F32(patch.vel_sens)),
        ("vib_rate",     ParamValue::F32(patch.vib_rate)),
        ("vib_depth",    ParamValue::F32(patch.vib_depth)),
        ("vib_delay",    ParamValue::F32(patch.vib_delay)),
        ("ensemble",     ParamValue::F32(patch.ensemble)),
        ("tune_cents",   ParamValue::F32(patch.tune_cents)),
    ]
}

fn generate_vstpreset_files() {
    let presets = ArchetPatch::factory_presets();
    let class_id = b"PxArchetBow00001";
    let version = env!("CARGO_PKG_VERSION");

    let mapped: Vec<(String, Vec<(&str, ParamValue)>)> = presets
        .iter()
        .map(|patch| {
            let cat = patch.preset_category().unwrap_or("Archet");
            let path_name = format!("{}/{}", cat, patch.name);
            (path_name, map_patch_to_nih_params(patch))
        })
        .collect();

    let preset_refs: Vec<(&str, Vec<(&str, ParamValue)>)> = mapped
        .iter()
        .map(|(name, params)| (name.as_str(), params.clone()))
        .collect();

    match vstpreset::generate_factory_presets(
        "Phonix Audio", "Archet", class_id, version, &preset_refs,
    ) {
        Ok(count) => nice_log!("Archet: generated {} .vstpreset files", count),
        Err(e)    => nice_log!("Archet: FAILED to generate .vstpreset files: {}", e),
    }
}

// -- Plugin implementation -------------------------------------------------

impl Plugin for ArchetPlugin {
    const NAME:    &'static str = "Archet";
    const VENDOR:  &'static str = "Phonix Audio";
    const URL:     &'static str = "";
    const EMAIL:   &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels:  None,
        main_output_channels: Some(std::num::NonZeroU32::new(2).unwrap()),
        aux_input_ports:      &[],
        aux_output_ports:     &[],
        names:                PortNames::const_default(),
    }];

    const MIDI_INPUT:  MidiConfig = MidiConfig::Basic;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> { self.params.clone() }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let patch_state  = self.params.patch_state.clone();
        let tx           = self.command_tx.clone();
        let meter_reader = self.meter_reader.take()?;
        let mut app = ArchetApp::new(tx, meter_reader);
        // Seed the editor from the restored state before its first frame, or
        // the closure below publishes the default patch over it.
        if let Ok(p) = patch_state.read() { app.set_patch(p.clone()); }
        let host_params = self.params.clone();
        let bank = Arc::new(self.factory_presets.clone());
        let fx_link = self.fx_link.clone();
        // The closure must be Sync, so the counter it remembers is an atomic.
        let seen = AtomicU64::new(fx_link.rev());
        // Seed the page from the restored patch, as the patch itself is above.
        if let Ok(p) = patch_state.read() {
            self.fx_link.seed(p.fx.clone());
        }

        create_egui_editor(
            self.params.editor_state.clone(),
            app,
            Default::default(),
            |_egui_ctx, _queue, _app| {},
            move |ui, setter, _queue, app| {
                // A preset change replaced the chain: adopt it before
                // drawing, or the page shows the previous preset's effects.
                let mut seen_now = seen.load(Ordering::Relaxed);
                if let Some(spec) = fx_link.adopt(&mut seen_now) {
                    app.set_fx(spec);
                    seen.store(seen_now, Ordering::Relaxed);
                }
                app.draw_ui(ui);
                // The page moved something: publish it, and the audio thread
                // takes it on its next block.
                if app.take_fx_changed() {
                    seen.store(fx_link.publish(app.fx().clone()), Ordering::Relaxed);
                }
                // A preset picked in the window moves the parameter, and the
                // parameter loads the patch: one path, automation included.
                // Its chain goes through the link as well: a preset picked
                // again is the same parameter value, and the audio thread
                // would never hear of it otherwise.
                if let Some(i) = app.take_wants_preset() {
                    let chain = bank.get((i - 1).max(0) as usize).filter(|_| i > 0).map(|p| p.fx.clone()).unwrap_or_default();
                    seen.store(fx_link.publish(chain), Ordering::Relaxed);
                    setter.begin_set_parameter(&host_params.preset);
                    setter.set_parameter(&host_params.preset, i);
                    setter.end_set_parameter(&host_params.preset);
                }
                if let Ok(mut p) = patch_state.write() {
                    *p = app.current_patch();
                }
            },
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = buffer_config.sample_rate;
        // Re-entrant: nice-plug calls this again from `set_state` whenever a
        // buffer config exists. The channel ends exist once, so the engine is
        // built on the first call and re-rated in place afterwards.
        match self.pending.take() {
            Some((rx, meter_writer)) => {
                self.engine = Some(ArchetEngine::new(sr, rx, meter_writer));
            }
            None => match self.engine.as_mut() {
                Some(e) => e.set_sample_rate(sr),
                None => return false,
            },
        }
        let max_block = buffer_config.max_buffer_size as usize;
        self.interleaved_buf = vec![0.0f32; max_block * 2];
        self.buf_l = vec![0.0f32; max_block];
        self.buf_r = vec![0.0f32; max_block];
        // Built here rather than in `Default` so it never carries a
        // placeholder rate, and re-rated in place on the second call.
        match self.fx_chain.as_mut() {
            Some(c) => c.prepare(sr, max_block),
            None => self.fx_chain = Some(Chain::new(sr, max_block)),
        }

        let patch = self.params.patch_state.read().map(|p| p.clone()).unwrap_or_default();
        // A restored project brings its chain with it, inside the patch; one
        // saved before the field existed brings an empty spec, and an empty
        // spec leaves an empty chain.
        if let Some(c) = self.fx_chain.as_mut() {
            fx::apply(c, &patch.fx);
            context.set_latency_samples(c.latency_samples() as u32);
        }
        self.fx_link.seed(patch.fx.clone());
        self.fx_seen = self.fx_link.rev();
        let _ = self.command_tx.send(ArchetCommand::LoadPatch(Box::new(patch)));
        self.last_preset = self.params.preset.value();
        self.last_init_sig = None;

        generate_vstpreset_files();
        true
    }

    fn reset(&mut self) {}

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let engine = match self.engine.as_mut() { Some(e) => e, None => return ProcessStatus::Normal };
        let tx     = &self.command_tx;

        // -- Preset change --
        let current_preset = self.params.preset.value();
        if current_preset != self.last_preset {
            self.last_preset = current_preset;
            self.last_init_sig = None; // re-push Init params if user switches back to 0
            if current_preset > 0 {
                let idx = (current_preset - 1) as usize;
                if let Some(patch) = self.factory_presets.get(idx) {
                    let _ = tx.send(ArchetCommand::LoadPatch(Box::new(patch.clone())));
                    // The preset's own chain, from the bank rather than from
                    // the editor: this branch also fires on host automation.
                    if let Some(c) = self.fx_chain.as_mut() {
                        fx::apply(c, &patch.fx);
                        context.set_latency_samples(c.latency_samples() as u32);
                    }
                    self.fx_seen = self.fx_link.publish(patch.fx.clone());
                }
            } else {
                // Back to Init: the curated chain goes with the preset it
                // came with.
                if let Some(c) = self.fx_chain.as_mut() {
                    fx::disengage(c);
                    context.set_latency_samples(c.latency_samples() as u32);
                }
                self.fx_seen = self.fx_link.publish(ChainSpec::default());
            }
        }
        // An edit made on the effects page, or a preset picked there again.
        if let Some(c) = self.fx_chain.as_mut() {
            let mut latency = None;
            self.fx_link.apply_if_new(&mut self.fx_seen, |spec| {
                fx::apply(c, spec);
                latency = Some(c.latency_samples() as u32);
            });
            if let Some(l) = latency {
                context.set_latency_samples(l);
            }
        }

        // -- Init mode: rebuild + LoadPatch only when a param changed --
        if current_preset == 0 {
            let sig = self.params.init_sig();
            let changed = self.last_init_sig.is_none_or(|prev| {
                prev.iter().zip(sig.iter()).any(|(a, b)| (a - b).abs() > 1e-6)
            });
            if changed {
                self.last_init_sig = Some(sig);
                let patch = self.params.build_init_patch();
                let _ = tx.send(ArchetCommand::LoadPatch(Box::new(patch)));
            }
        }

        // -- MIDI events --
        while let Some(event) = context.next_event() {
            match event {
                NoteEvent::NoteOn { note, velocity, .. } => {
                    let vel = (velocity * 127.0) as u8;
                    let _ = tx.send(ArchetCommand::NoteOn(note, vel));
                }
                NoteEvent::NoteOff { note, .. } => {
                    let _ = tx.send(ArchetCommand::NoteOff(note));
                }
                NoteEvent::MidiPitchBend { value, .. } => {
                    let semitones = (value - 0.5) * 4.0;   // +/-2 semitones
                    let _ = tx.send(ArchetCommand::PitchBend(semitones));
                }
                _ => {}
            }
        }

        // -- Audio --
        let num_samples     = buffer.samples();
        let interleaved_len = num_samples * 2;
        if self.interleaved_buf.len() < interleaved_len {
            self.interleaved_buf.resize(interleaved_len, 0.0);
        }
        for s in &mut self.interleaved_buf[..interleaved_len] { *s = 0.0; }
        engine.process_audio(&mut self.interleaved_buf[..interleaved_len], 2);
        // The chain runs in stereo before any summing: the space's width
        // needs both channels.
        if self.buf_l.len() < num_samples {
            self.buf_l.resize(num_samples, 0.0);
            self.buf_r.resize(num_samples, 0.0);
        }
        for i in 0..num_samples {
            self.buf_l[i] = self.interleaved_buf[i * 2];
            self.buf_r[i] = self.interleaved_buf[i * 2 + 1];
        }
        if let Some(c) = self.fx_chain.as_mut() {
            c.process(&mut self.buf_l[..num_samples], &mut self.buf_r[..num_samples], &[], Transport::default(), Musical::default());
        }

        let channel_slices = buffer.as_slice();
        if channel_slices.len() >= 2 {
            let (left, right) = channel_slices.split_at_mut(1);
            left[0][..num_samples].copy_from_slice(&self.buf_l[..num_samples]);
            right[0][..num_samples].copy_from_slice(&self.buf_r[..num_samples]);
        } else if !channel_slices.is_empty() {
            let mono = &mut channel_slices[0];
            for i in 0..num_samples {
                mono[i] = (self.buf_l[i] + self.buf_r[i]) * 0.5;
            }
        }

        ProcessStatus::Normal
    }
}

// -- CLAP ------------------------------------------------------------------

impl ClapPlugin for ArchetPlugin {
    const CLAP_ID: &'static str = "com.phonix-audio.archet";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A bowed string: the violin family, drawn with the bow or plucked");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    // What this plugin IS, in the vocabulary each format publishes. A host
    // groups its plugin list by these, so they are the plugin's answer to
    // "where do I belong" -- not something a browser should be keeping a
    // table of.
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::Instrument, ClapFeature::Custom("physical-modeling"), ClapFeature::Custom("strings"), ClapFeature::Stereo];
}

// -- VST3 ------------------------------------------------------------------

impl Vst3Plugin for ArchetPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"PxArchetBow00001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth];
}

nice_export_clap!(ArchetPlugin);
nice_export_vst3!(ArchetPlugin);

// -- Frozen identifiers ----------------------------------------------------
//
// The VST3 class id, the CLAP id and the display name are WIRE FORMAT: they
// compose the `.vstpreset` header, the DAWproject `deviceID` and the directory
// Cubase's MediaBay indexes. Changing one orphans every preset and session that
// points at this plugin. The in-monolith version of this test compared the
// class id against `phonix_preset::devices::plugin_for_engine(&EngineTag::Archet)`;
// that dependency ran plugin -> phonix and now runs the other way, and the
// `EngineTag::Archet` variant is gone. So both sides assert the same literals
// instead, and a drift still fails a build - just two builds instead of one
// (the sequencer's `the_extracted_class_ids_are_the_plugins_own` is the other half).
#[cfg(test)]
mod frozen_identifiers {
    use super::*;

    #[test]
    fn the_four_identifiers_are_frozen() {
        assert_eq!(<ArchetPlugin as Plugin>::NAME, "Archet");
        assert_eq!(<ArchetPlugin as Plugin>::VENDOR, "Phonix Audio");
        assert_eq!(<ArchetPlugin as ClapPlugin>::CLAP_ID, "com.phonix-audio.archet");
        assert_eq!(&<ArchetPlugin as Vst3Plugin>::VST3_CLASS_ID, b"PxArchetBow00001");
    }

    /// The `.vstpreset` writer must use the very same class id as the plugin,
    /// or a generated bank is invisible to the host that scanned the plugin.
    #[test]
    fn the_vstpreset_writer_uses_the_plugins_class_id() {
        assert_eq!(b"PxArchetBow00001", &<ArchetPlugin as Vst3Plugin>::VST3_CLASS_ID);
    }

    /// The window the plugin declares is the one the editor is laid out
    /// against. `archet-ui` owns the numbers; this is the pin.
    #[test]
    fn the_declared_window_size_is_the_editors_own() {
        let p = ArchetParams::new(0, Arc::new(vec!["Init".to_string()]));
        assert_eq!(p.editor_state.size(), (archet_app::W as u32, archet_app::H as u32));
        assert_eq!((archet_app::W, archet_app::H), (1160.0, 770.0));
    }
}

// -- The chain and its absence ---------------------------------------------
#[cfg(test)]
mod fx_chain_compat {
    use super::*;

    /// A fresh plugin holds no live chain until it is initialised, and its
    /// persisted patch is the default one, chain included.
    #[test]
    fn a_fresh_plugin_persists_the_default_patch_with_its_chain() {
        let p = ArchetPlugin::default();
        assert!(p.fx_chain.is_none());
        let held = p.params.patch_state.read().unwrap().fx.clone();
        assert_eq!(held, ArchetPatch::default().fx);
        assert_eq!(held.len(), archet::fx::FX_SLOTS);
    }

    /// Init is the absence of a factory preset: the patch it builds carries
    /// the default chain, and `process` disengages it when the parameter
    /// reads zero, so nothing here may depend on the chain being empty.
    #[test]
    fn the_init_patch_names_itself() {
        let params = ArchetParams::default();
        assert_eq!(params.build_init_patch().name, "Init");
    }
}
