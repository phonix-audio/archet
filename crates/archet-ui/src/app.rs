//! Archet GUI — bowed-string / harpsichord physical model editor.
//!
//! A single-panel knob editor (no tabs): the model has ~24 flat parameters
//! grouped into Bow, String/Body, Articulation, Vibrato and Section sections.
//! Archet exposes no per-parameter Set commands (only NoteOn/Off, polyphony,
//! output and `LoadPatch`), so any knob edit pushes the whole patch via
//! `LoadPatch` — which is non-destructive in the engine (it stores the patch
//! and flags it dirty, it never kills active voices, so dragging a knob over a
//! held note does not click).

use std::sync::mpsc;
use archet::state_buffer::SharedReader;
use egui::Ui;

use phonix_ui::icons;
use archet::engine::{ArchetCommand, ArchetMeterState};
use archet::patch::{ArchetPatch, FrictionKind};
use phonix_ui::colors::*;

// ── The window size, in ONE place ─────────────────────────────────────────
//
// The plugin's `EguiState::from_size` and the size the editor is actually laid
// out against used to be two independent numbers, and across the family they
// disagreed often enough to ship editors that opened CROPPED. So the editor
// crate owns the size, the plugin reads these constants, and
// `archet_plugin`'s `the_declared_window_size_is_the_editors_own` pins the two
// together.
//
// 1100x720 is the size `plugins/archet` declared in the monolith and it is
// what the standalone plugin keeps. Note that the monolith's SEQUENCER opened
// this editor smaller, at 840x560 (`plugin_window.rs`'s display-name-keyed
// row, deleted with the extraction) — so unlike the seven cropped plugins the
// gap here runs the safe way: the hosted editor is now roomier, never cut off.
// See COMPAT.md.
pub const W: f32 = 1100.0;
pub const H: f32 = 720.0;

pub struct ArchetApp {
    /// Declarative body layout (hot-reloaded in debug builds).
    spec: phonix_ui::ui_spec::SpecHandle,
    spec_tab: usize,
    tx:           mpsc::Sender<ArchetCommand>,
    meter_reader: SharedReader<ArchetMeterState>,
    cached_meter: ArchetMeterState,

    patch: ArchetPatch,

    kb: phonix_ui::keyboard::KeyboardState,

    presets:      Vec<ArchetPatch>,
    preset_state: phonix_ui::preset_picker::PresetPickerState,

    peak_l:        f32,
    peak_r:        f32,
    voice_count:   usize,
    sync_cooldown: u8,
}

impl ArchetApp {
    pub fn new(tx: mpsc::Sender<ArchetCommand>, meter_reader: SharedReader<ArchetMeterState>) -> Self {
        Self {
            spec: phonix_ui::ui_spec::SpecHandle::load(
                "archet", include_str!("../ui_specs/archet.ron")),
            spec_tab: 0,
            tx, meter_reader, cached_meter: ArchetMeterState::default(),
            patch: ArchetPatch::default(),
            kb: phonix_ui::keyboard::KeyboardState::new(4, 3),
            presets: ArchetPatch::factory_presets(),
            preset_state: phonix_ui::preset_picker::PresetPickerState::default(),
            peak_l: 0.0, peak_r: 0.0, voice_count: 0,
            sync_cooldown: 0,
        }
    }

    fn send(&self, cmd: ArchetCommand) { let _ = self.tx.send(cmd); }


    /// Patch the editor currently displays — used by the VST3/CLAP editor to
    /// persist GUI edits with the project.
    pub fn current_patch(&self) -> ArchetPatch { self.patch.clone() }

    fn load_preset(&mut self, idx: usize) {
        if let Some(p) = self.presets.get(idx) {
            self.patch = p.clone();
            self.send(ArchetCommand::LoadPatch(Box::new(p.clone())));
            self.sync_cooldown = 10;
        }
    }
}

impl ArchetApp {

    /// Draw the declarative body and turn each changed field into ONE typed
    /// command.
    fn draw_body(&mut self, ui: &mut Ui,
                 fx: Option<&mut dyn phonix_fx_ui::fx_rack::FxRackBackend>) {
        self.spec.poll();
        let mut access = phonix_ui::ui_spec::JsonPatchAccess::new(&self.patch);
        let mut custom = phonix_ui::ui_spec::CustomTable::new();
        phonix_ui::ui_spec::render_spec(
            ui, self.spec.spec(), &mut access, &mut custom, &mut self.spec_tab);
        if self.spec.spec().tabs.get(self.spec_tab).is_some_and(|t| t.label == "FX") {
            phonix_fx_ui::fx_host::draw_standalone_rack(ui, None, fx,
                &phonix_fx_ui::fx_rack::FxRackConfig {
                    label: "Insert FX", id_salt: "archet_track_fx", knob_size: 32.0 });
        }
        for field in phonix_ui::ui_spec::PatchAccess::take_changed(&mut access) {
            let Some(np) = access.patch_view::<ArchetPatch>() else { continue };
            let cmd = Self::command_for(&field, &np);
            self.patch = np;
            if let Some(c) = cmd {
                self.send(c);
                // The engine echoes its patch through the meter channel; hold
                // the mirror off for a few frames so a drag is not fought by a
                // stale snapshot.
                self.sync_cooldown = 10;
            }
        }
    }

    /// One changed field -> one typed command.
    ///
    /// Everything that is not already a dedicated setter travels as
    /// `SetParam`, which writes the single field and leaves the sympathetic
    /// strings and the voice pool alone.
    fn command_for(field: &str, p: &ArchetPatch) -> Option<ArchetCommand> {
        use archet::patch::ArchetParam as P;
        use ArchetCommand as C;
        Some(match field {
            "output_level" => C::SetOutputLevel(p.output_level),
            "polyphony"    => C::SetPolyphony(p.polyphony),
            _ => {
                let (param, value) = match field {
                    "bow_pos"        => (P::BowPos, p.bow_pos),
                    "bow_vel"        => (P::BowVel, p.bow_vel),
                    "bow_force"      => (P::BowForce, p.bow_force),
                    "bow_noise"      => (P::BowNoise, p.bow_noise),
                    "loss"           => (P::Loss, p.loss),
                    "slope"          => (P::Slope, p.slope),
                    "bridge_hill_db" => (P::BridgeHillDb, p.bridge_hill_db),
                    "tor_ratio"      => (P::TorRatio, p.tor_ratio),
                    "tor_couple"     => (P::TorCouple, p.tor_couple),
                    "tor_inject"     => (P::TorInject, p.tor_inject),
                    "attack"         => (P::Attack, p.attack),
                    "release"        => (P::Release, p.release),
                    "vel_sens"       => (P::VelSens, p.vel_sens),
                    "vib_rate"       => (P::VibRate, p.vib_rate),
                    "vib_depth"      => (P::VibDepth, p.vib_depth),
                    "vib_delay"      => (P::VibDelay, p.vib_delay),
                    "ensemble"       => (P::Ensemble, p.ensemble),
                    "tune_cents"     => (P::TuneCents, p.tune_cents),
                    "friction"       => (P::Friction,
                        if p.friction == FrictionKind::ElastoPlastic { 1.0 } else { 0.0 }),
                    "instrument"     => (P::Instrument, p.instrument.to_index() as f32),
                    "auto_range"     => (P::AutoRange, if p.auto_range { 1.0 } else { 0.0 }),
                    "pluck"          => (P::Pluck, if p.pluck { 1.0 } else { 0.0 }),
                    _ => return None,
                };
                C::SetParam { param, value }
            }
        })
    }

    pub fn draw_ui(&mut self, ctx: &egui::Context) { self.draw_ui_inner(ctx, None) }

    /// Draw with the track's insert rack, editable from the instrument's FX tab.
    pub fn draw_ui_with_fx(&mut self, ctx: &egui::Context,
                           fx: &mut dyn phonix_fx_ui::fx_rack::FxRackBackend) {
        self.draw_ui_inner(ctx, Some(fx))
    }

    fn draw_ui_inner(&mut self, ctx: &egui::Context,
                     fx: Option<&mut dyn phonix_fx_ui::fx_rack::FxRackBackend>) {
        icons::install(ctx);
        if let Ok(mut r) = self.meter_reader.try_lock() {
            if let Some(fresh) = r.read() { self.cached_meter.clone_from(fresh); }
        }
        self.peak_l = self.cached_meter.peak_l;
        self.peak_r = self.cached_meter.peak_r;
        self.voice_count = self.cached_meter.voice_count;
        if self.sync_cooldown > 0 {
            self.sync_cooldown -= 1;
        } else if let Some(snap) = self.cached_meter.patch_snapshot.clone() {
            self.patch = snap;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        // ── Header (shared chrome panel + preset picker) ─────────────────
        self.preset_state.sync_to_name(&self.presets, &self.patch.name);
        {
            let peak = self.peak_l.max(self.peak_r);
            let status = format!("{} voices", self.voice_count);
            let res = phonix_ui::widgets::plugin_chrome_panel(ctx,
                &phonix_ui::widgets::PluginChrome {
                    title: "ARCHET", accent: ACCENT_ARCHET, dim: TEXT_DIM,
                    peak, cpu: Some(self.cached_meter.cpu_percent / 100.0), preset_salt: "arc_preset",
                    mode_pills: &[],
                    status_right: Some(&status),
                },
                &mut self.preset_state, &self.presets);
            if let Some(i) = res.preset_selected { self.load_preset(i); }
            if res.save_clicked {
                phonix_ui::preset_io::save_patch_to_disk(&self.patch, "Archet", &self.patch.name);
            }
            if res.load_clicked {
                if let Some(p) = phonix_ui::preset_io::load_patch_from_disk::<ArchetPatch>("Archet") {
                    self.patch = p.clone();
                    self.send(ArchetCommand::LoadPatch(Box::new(p)));
                }
            }
        }

        // ── On-screen keyboard for auditioning ───────────────────────────
        // Declared BEFORE the central panel so egui reserves its space and it
        // stays pinned at the bottom (central must be added last).
        egui::Panel::bottom("archet_keyboard").show(ctx, |ui| {
            self.draw_keyboard(ui);
        });

        // ── Body: parameter sections ─────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            self.draw_body(ui, fx);
        });
    }



    fn draw_keyboard(&mut self, ui: &mut Ui) {
        self.kb.active.clone_from(&self.cached_meter.active_notes);
        let style = phonix_ui::keyboard::KeyboardStyle {
            accent: ACCENT_ARCHET, id_salt: "archet_kbd", ..Default::default()
        };
        for ev in phonix_ui::keyboard::keyboard_ui(ui, &mut self.kb, &style) {
            match ev {
                phonix_ui::keyboard::KeyEvent::On { note, velocity } =>
                    self.send(ArchetCommand::NoteOn(note, velocity)),
                phonix_ui::keyboard::KeyEvent::Off { note } =>
                    self.send(ArchetCommand::NoteOff(note)),
                // Wheels are off for this plugin (no PB/MW commands wired yet).
                phonix_ui::keyboard::KeyEvent::PitchBend(_)
                | phonix_ui::keyboard::KeyEvent::ModWheel(_) => {}
            }
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────





#[cfg(test)]
mod tests {
    use super::*;
    use archet::state_buffer::meter_channel;
    use egui_kittest::Harness;

    /// The declared window size is the size the editor is laid out against.
    ///
    /// The failure this pins is the one that shipped seven cropped editors: a
    /// plugin declaring a window smaller than its own composition needs. Render
    /// at exactly `(W, H)` and assert egui laid everything out inside it.
    #[test]
    fn the_editor_fits_the_declared_window_size() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let (_mw, mr) = meter_channel::<ArchetMeterState>();
        let mut app = ArchetApp::new(tx, mr);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(W, H))
            .build(move |ctx| {
                egui_extras::install_image_loaders(ctx);
                app.draw_ui(ctx);
            });
        harness.run_steps(2);
        let used = harness.ctx.used_rect();
        assert!(
            used.width() <= W + 0.5 && used.height() <= H + 0.5,
            "the editor lays out {}x{} but the plugin declares {W}x{H} — a host \
             would open it cropped",
            used.width(), used.height(),
        );
    }

    /// Every control in archet.ron names a field that exists in ArchetPatch.
    #[test]
    fn the_archet_spec_matches_the_patch() {
        let spec: phonix_ui::ui_spec::PluginUiSpec =
            ron::from_str(include_str!("../ui_specs/archet.ron")).expect("archet.ron parses");
        let sample = serde_json::to_value(ArchetPatch::default()).unwrap();
        let errs = phonix_ui::ui_spec::validate(&spec, &sample);
        assert!(errs.is_empty(), "archet.ron does not match ArchetPatch:\n{}", errs.join("\n"));
    }

    /// Every spec param maps to a typed command, or the knob is inert.
    #[test]
    fn every_archet_control_maps_to_a_command() {
        let spec: phonix_ui::ui_spec::PluginUiSpec =
            ron::from_str(include_str!("../ui_specs/archet.ron")).unwrap();
        let p = ArchetPatch::default();
        let mut orphan = Vec::new();
        for tab in &spec.tabs {
            for sec in tab.sections.iter().chain(tab.bands.iter().flat_map(|b| &b.sections)) {
                for row in &sec.rows {
                    for c in row {
                        if c.param.is_empty() { continue }
                        if ArchetApp::command_for(&c.param, &p).is_none() {
                            orphan.push(c.param.clone());
                        }
                    }
                }
            }
        }
        assert!(orphan.is_empty(), "spec params with no command: {orphan:?}");
    }

    /// And no typed setter is unreachable from the layout.
    #[test]
    fn no_archet_setter_is_unreachable() {
        let missing = phonix_ui::ui_spec::uncovered_setters(
            include_str!("../ui_specs/archet.ron"),
            include_str!("../../archet/src/engine.rs"),
            "Set",
            &[
                // Every layout field travels as one of these, named by
                // `ArchetParam`; the spec addresses the FIELDS, not the command.
                "Param",
            ],
        );
        assert!(missing.is_empty(), "archet.ron reaches no control for: {missing:?}");
    }

    /// A `SetParam` writes ONE field and leaves everything else alone.
    ///
    /// The point of the whole exercise: a drag frame used to arrive as a full
    /// patch, which walks the voice pool and can rebuild the sympathetic
    /// strings under a sounding note.
    #[test]
    fn a_param_edit_touches_only_that_field() {
        use archet::patch::ArchetParam;
        let mut p = ArchetPatch::default();
        let before = p.clone();
        ArchetParam::BowForce.apply(&mut p, 1.75);
        assert!((p.bow_force - 1.75).abs() < 1e-6);
        assert_eq!(p.bow_pos, before.bow_pos);
        assert_eq!(p.instrument, before.instrument);
        assert_eq!(p.seed_offset, before.seed_offset);
        assert_eq!(p.ensemble, before.ensemble);
    }

    /// Only the body choice may force a sympathetic-string rebuild.
    #[test]
    fn only_the_body_choice_rebuilds_the_sympathetics() {
        use archet::patch::ArchetParam as P;
        for p in [P::Instrument, P::AutoRange, P::Pluck] {
            assert!(p.needs_symp_rebuild(), "{p:?} must rebuild the sympathetics");
        }
        for p in [P::BowPos, P::BowVel, P::Loss, P::VibRate, P::Ensemble, P::TuneCents] {
            assert!(!p.needs_symp_rebuild(), "{p:?} must NOT rebuild the sympathetics");
        }
    }

    /// The instrument index table is the GUI/audio wire format: round trip it.
    #[test]
    fn the_instrument_index_table_round_trips() {
        use archet::patch::Instrument;
        for (i, inst) in Instrument::ALL_ORDERED.iter().enumerate() {
            assert_eq!(Instrument::from_index(i), *inst);
            assert_eq!(inst.to_index(), i);
        }
    }

    #[test]
    #[ignore]
    fn archet_app_snapshot() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let (_mw, mr) = meter_channel::<ArchetMeterState>();
        let mut app = ArchetApp::new(tx, mr);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(840.0, 560.0))
            .build(move |ctx| {
                egui_extras::install_image_loaders(ctx);
                app.draw_ui(ctx);
            });
        harness.run_steps(2);
        harness.snapshot("archet_app");
    }
}
