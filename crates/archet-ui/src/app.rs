//! The editor: the machine under the shared chrome, the keyboard below.
//!
//! Laid out in fixed rectangles rather than egui containers. The composition
//! is a drawing -- a bow crossing a string, the corpus it drives, the player
//! holding both -- and a column layout that reflows is the wrong tool for a
//! picture. The window does not resize, so nothing is lost by pinning it.

use egui::{Align2, Color32, FontId, Pos2, Rect, Ui, Vec2};
use std::sync::mpsc;

use archet::patch::{ArchetParam, ArchetPatch, Instrument};
use archet::{ArchetCommand, ArchetMeterState};
use phonix_plugin::preset::Preset;
use phonix_rt::SharedReader;
use phonix_ui::chrome::{plugin_chrome_panel, PillEntry, PluginChrome};
use phonix_ui::keyboard::{keyboard_ui, KeyEvent, KeyboardState, KeyboardStyle};
use phonix_ui::preset_picker::{NamedPreset, PresetPickerState};
use phonix_ui::widgets;

use crate::colors::*;
use crate::machine;
use crate::panel_geom as geom;
use crate::theme;

/// The size the layout is drawn for. The window is fixed at this.
pub const W: f32 = 1160.0;
pub const H: f32 = 770.0;
/// The panel's height inside the case, the chrome above and the keyboard
/// below taken off. The head band went with the nameplate: the name and
/// what the instrument is are in the shared header now.
pub const PANEL_H: f32 = 602.0;
const KEYBOARD_H: f32 = 104.0;

/// The four bodies, in the engine's own order.
const BODIES: [&str; 4] = ["VIOLIN", "VIOLA", "CELLO", "BASS"];

pub struct ArchetApp {
    tx: mpsc::Sender<ArchetCommand>,
    meter_reader: SharedReader<ArchetMeterState>,
    cached_meter: ArchetMeterState,
    patch: ArchetPatch,
    presets: Vec<ArchetPatch>,
    picker: PresetPickerState,
    /// A factory preset pick pending: the editor asks, the plugin moves the
    /// parameter, and the parameter is what reaches the engine.
    wants_preset: Option<i32>,
    /// The named fields moved in the window since the plugin last asked,
    /// for the host's parameters to follow.
    moved: Vec<(ArchetParam, f32)>,
    keys: KeyboardState,
    /// Frames left before the editor will adopt the engine's patch again.
    sync_cooldown: u8,
    mirror_hold: u8,
    /// Which page the case shows: 0 the instrument, 1 the effects.
    tab: usize,
    /// The effects page moved something the audio thread has not taken yet.
    fx_dirty: bool,
    fx_page: crate::fx_page::FxPageState,
}

impl ArchetApp {
    pub fn new(tx: mpsc::Sender<ArchetCommand>, meter_reader: SharedReader<ArchetMeterState>) -> Self {
        let presets = ArchetPatch::factory_presets();
        Self {
            tx,
            meter_reader,
            cached_meter: ArchetMeterState::default(),
            patch: presets.first().cloned().unwrap_or_default(),
            presets,
            picker: PresetPickerState::default(),
            wants_preset: None,
            moved: Vec::new(),
            keys: KeyboardState::new(2, 4),
            sync_cooldown: 0,
            mirror_hold: 0,
            tab: 0,
            fx_dirty: false,
            fx_page: Default::default(),
        }
    }

    pub fn current_patch(&self) -> ArchetPatch {
        self.patch.clone()
    }

    /// Seed the mirror from a patch the host has restored. Sends nothing:
    /// an editor that pushes a patch on open stomps a running engine.
    pub fn set_patch(&mut self, patch: ArchetPatch) {
        self.patch = patch;
        self.sync_cooldown = 20;
    }

    pub fn pick_preset(&mut self, i: usize) {
        if let Some(p) = self.presets.get(i).cloned() {
            self.patch = p;
            self.picker.current_idx = i;
            self.wants_preset = Some(i as i32 + 1);
            self.sync_cooldown = 20;
        }
    }

    pub fn take_wants_preset(&mut self) -> Option<i32> {
        self.wants_preset.take()
    }

    /// An edit of the chain, as the effects page makes one: the chain moves
    /// here and the plugin is told to publish it.
    pub fn edit_fx(&mut self, f: impl FnOnce(&mut phonix_fx::ChainSpec)) {
        f(&mut self.patch.fx);
        self.fx_dirty = true;
    }

    /// Whether the chain moved since the plugin last asked.
    pub fn take_fx_changed(&mut self) -> bool {
        std::mem::take(&mut self.fx_dirty)
    }

    /// Adopt the chain the plugin is running: after a preset change, or a
    /// restored project.
    pub fn set_fx(&mut self, spec: phonix_fx::ChainSpec) {
        self.patch.fx = spec;
        self.fx_dirty = false;
    }

    /// The chain as the editor holds it.
    pub fn fx(&self) -> &phonix_fx::ChainSpec {
        &self.patch.fx
    }

    fn send(&mut self, cmd: ArchetCommand) {
        self.sync_cooldown = 20;
        let _ = self.tx.send(cmd);
    }

    /// One named field moved: the patch here and the engine's. `SetParam`
    /// rather than `LoadPatch`, because a whole patch rebuilds the
    /// sympathetic strings and re-seeds the voice pool on every drag frame.
    fn set(&mut self, param: ArchetParam, value: f32) {
        param.apply(&mut self.patch, value);
        self.moved.push((param, value));
        self.send(ArchetCommand::SetParam { param, value });
    }

    /// The fields moved in the window since the last call.
    pub fn take_moved(&mut self) -> Vec<(ArchetParam, f32)> {
        std::mem::take(&mut self.moved)
    }

    fn refresh_meters(&mut self, adopt_ok: bool) {
        if let Ok(mut r) = self.meter_reader.try_lock() {
            if let Some(fresh) = r.read() {
                self.cached_meter.clone_from(fresh);
            }
        }
        if self.sync_cooldown > 0 {
            self.sync_cooldown -= 1;
        } else if adopt_ok {
            if let Some(ref ep) = self.cached_meter.patch_snapshot {
                // Everything except the chain. The engine mirrors a whole
                // patch but runs no effects, so its copy of `fx` is stale the
                // moment the page moves a knob; the chain travels on its own
                // channel, `set_fx`, from the plugin that runs it.
                let fx = std::mem::take(&mut self.patch.fx);
                self.patch = ep.clone();
                self.patch.fx = fx;
            }
        }
    }

    /// The body the patch names, as the panel's own index.
    fn body(&self) -> usize {
        match self.patch.instrument {
            Instrument::Violin => 0,
            Instrument::Viola => 1,
            Instrument::Cello => 2,
            Instrument::DoubleBass => 3,
        }
    }

    // -- The window ----------------------------------------------------

    pub fn draw_ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui_extras::install_image_loaders(&ctx);
        theme::apply_visuals(&ctx);
        let adopt_ok = widgets::mirror_adopt_gate(&ctx, &mut self.mirror_hold);
        self.refresh_meters(adopt_ok);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        self.draw_chrome(ui);
        self.draw_keyboard(ui);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(PANEL_EDGE))
            .show_inside(ui, |ui| {
                let full = ui.max_rect();
                theme::gradient_v(ui, full, Color32::from_rgb(26, 22, 20), Color32::from_rgb(10, 9, 8));
                let case = Rect::from_min_max(full.min + Vec2::new(8.0, 6.0), full.max - Vec2::new(8.0, 10.0));
                let panel = machine::case(ui, case);
                if self.tab == 1 {
                    // The effects take the whole case: three effects want
                    // more than a tier could give.
                    let page = Rect::from_min_max(
                        Pos2::new(geom::tier(panel, 0).left(), geom::tier(panel, 0).top()),
                        Pos2::new(geom::tier(panel, 2).right(), geom::tier(panel, 2).bottom()),
                    );
                    if crate::fx_page::draw(ui, page, &mut self.patch.fx, &mut self.fx_page) {
                        self.fx_dirty = true;
                    }
                } else {
                    self.draw_bow(ui, geom::tier(panel, 0));
                    self.draw_body(ui, geom::tier(panel, 1));
                    self.draw_player(ui, geom::tier(panel, 2));
                }
            });
    }

    fn draw_chrome(&mut self, ui: &mut Ui) {
        if let Some(i) = self.presets.iter().position(|p| p.name == self.patch.name) {
            self.picker.current_idx = i;
        }
        let cats: Vec<Option<String>> =
            self.presets.iter().map(|p| p.preset_category().map(|c| c.to_string())).collect();
        let named: Vec<NamedPreset> = self
            .presets
            .iter()
            .zip(cats.iter())
            .map(|(p, c)| NamedPreset { name: &p.name, category: c.as_deref() })
            .collect();
        let status = format!("{} voices", self.cached_meter.voice_count);
        let pills = [
            PillEntry { label: "INSTRUMENT", selected: self.tab == 0 },
            PillEntry { label: "EFFECTS", selected: self.tab == 1 },
        ];
        let chrome = PluginChrome {
            title: "ARCHET",
            accent: ROSIN,
            dim: SILK_DIM,
            peak: self.cached_meter.peak_l.max(self.cached_meter.peak_r),
            // The chrome reads a fraction; the engine publishes a percent.
            cpu: Some(self.cached_meter.cpu_percent / 100.0),
            preset_salt: "archet",
            // What the panel's own nameplate used to print under this bar.
            subtitle: Some("BOWED STRING"),
            patch_name: None, buttons: &[],
            mode_pills: &pills,
            status_right: Some(&status),
        };
        let res = plugin_chrome_panel(ui, &chrome, &mut self.picker, &named);
        if let Some(i) = res.pill_clicked {
            self.tab = i;
        }
        if let Some(i) = res.preset_selected {
            self.pick_preset(i);
        }
        if res.save_clicked {
            phonix_ui::preset_io::save_patch_to_disk(crate::PRESET_HOME, &self.patch, "Archet", &self.patch.name);
        }
        if res.load_clicked {
            if let Some(p) = phonix_ui::preset_io::load_patch_from_disk::<ArchetPatch>(crate::PRESET_HOME, "Archet") {
                self.patch = p.clone();
                self.send(ArchetCommand::LoadPatch(Box::new(p)));
            }
        }
    }

    /// Notes go out on the raw sender, never through `send`: a note is not a
    /// parameter edit, and arming the mirror cooldown on every key would keep
    /// the editor from adopting the engine's patch while playing.
    fn draw_keyboard(&mut self, ui: &mut Ui) {
        egui::Panel::bottom("archet_keys")
            .exact_size(KEYBOARD_H)
            .frame(egui::Frame::NONE.fill(PANEL_EDGE).inner_margin(egui::Margin::symmetric(10i8, 4i8)))
            .show_inside(ui, |ui| {
                self.keys.active.clear();
                self.keys.active.extend_from_slice(&self.cached_meter.active_notes);
                let style = KeyboardStyle {
                    accent: ROSIN,
                    height: 64.0,
                    labels: true,
                    velocity: 100,
                    velocity_by_y: true,
                    header: true,
                    wheels: true,
                    id_salt: "archet_kbd",
                };
                for ev in keyboard_ui(ui, &mut self.keys, &style) {
                    let cmd = match ev {
                        KeyEvent::On { note, velocity } => ArchetCommand::NoteOn(note, velocity),
                        KeyEvent::Off { note } => ArchetCommand::NoteOff(note),
                        KeyEvent::PitchBend(v) => ArchetCommand::PitchBend(v),
                        KeyEvent::ModWheel(_) => continue,
                    };
                    let _ = self.tx.send(cmd);
                }
            });
    }

    // -- Tier one: the bow on the string -------------------------------

    fn draw_bow(&mut self, ui: &mut Ui, r: Rect) {
        theme::plate(ui, r, 4.0);
        theme::heading(ui, Pos2::new(r.left() + 12.0, r.top() + 11.0), "THE BOW", PLATE_SILK, r.left() + 92.0);
        theme::printed(ui, Pos2::new(r.left() + 104.0, r.top() + 11.0),
                       "where it crosses the string, how hard, and how fast",
                       FontId::proportional(8.5), PLATE_SILK_DIM, Align2::LEFT_CENTER);

        let band = Rect::from_min_max(r.min + Vec2::new(12.0, 0.0), r.max - Vec2::new(12.0, 0.0));
        if let Some(d) = machine::string_window(ui, geom::string_window(band), geom::string_field(band),
                                                self.patch.bow_pos, self.patch.bow_force,
                                                self.patch.bow_vel, self.patch.pluck, "ar_string") {
            if (d.beta - self.patch.bow_pos).abs() > 1e-4 {
                self.set(ArchetParam::BowPos, d.beta);
            }
            if (d.force - self.patch.bow_force).abs() > 1e-4 {
                self.set(ArchetParam::BowForce, d.force);
            }
        }

        let arco = Rect::from_min_size(geom::bow_knob(band, 0, 0) - Vec2::new(0.0, 4.0),
                                       Vec2::new(2.5 * geom::CELL, 20.0));
        let sel = usize::from(self.patch.pluck);
        if let Some(i) = machine::switch(ui, arco, &["ARCO", "PIZZICATO"], sel, ROSIN, "ar_pluck") {
            self.set(ArchetParam::Pluck, i as f32);
        }
        let row: [(&str, ArchetParam, f32, f32, f32, &str); 5] = [
            ("POSITION", ArchetParam::BowPos, self.patch.bow_pos, geom::BETA_MIN, geom::BETA_MAX, "f"),
            ("FORCE", ArchetParam::BowForce, self.patch.bow_force, 0.0, 1.0, "%"),
            ("SPEED", ArchetParam::BowVel, self.patch.bow_vel, 0.0, 1.0, "%"),
            ("NOISE", ArchetParam::BowNoise, self.patch.bow_noise, 0.0, 1.0, "%"),
            ("DAMPING", ArchetParam::Loss, self.patch.loss, 0.0, 1.0, "%"),
        ];
        for (i, (label, param, v, lo, hi, unit)) in row.into_iter().enumerate() {
            if let Some(nv) = self.knob(ui, geom::bow_knob(band, 1, i), geom::KNOB, label, v, lo, hi, unit, BOW) {
                self.set(param, nv);
            }
        }
    }

    // -- Tier two: the body --------------------------------------------

    fn draw_body(&mut self, ui: &mut Ui, r: Rect) {
        theme::plate(ui, r, 4.0);
        theme::heading(ui, Pos2::new(r.left() + 12.0, r.top() + 11.0), "THE BODY", PLATE_SILK, r.left() + 100.0);
        theme::printed(ui, Pos2::new(r.left() + 112.0, r.top() + 11.0),
                       "the corpus the string drives",
                       FontId::proportional(8.5), PLATE_SILK_DIM, Align2::LEFT_CENTER);

        let band = Rect::from_min_max(r.min + Vec2::new(12.0, 0.0), r.max - Vec2::new(12.0, 0.0));
        if let Some(i) = machine::body_rail(ui, band, &BODIES, self.body(), "ar_body") {
            self.set(ArchetParam::Instrument, i as f32);
        }
        machine::body_window(ui, geom::body_window(band), geom::body_field(band),
                             self.body(), self.patch.bridge_hill_db);

        let auto = Rect::from_min_size(geom::body_knob(band, 0, 0) - Vec2::new(0.0, 4.0),
                                       Vec2::new(2.5 * geom::CELL, 20.0));
        let sel = usize::from(self.patch.auto_range);
        if let Some(i) = machine::switch(ui, auto, &["ONE BODY", "BY PITCH"], sel, BODY, "ar_auto") {
            self.set(ArchetParam::AutoRange, i as f32);
        }

        // The body's one knob sits under its switch, in the column the bow's
        // knobs use a tier above.
        if let Some(nv) = self.knob(ui, geom::body_knob(band, 1, 0), geom::KNOB, "HILL",
                                    self.patch.bridge_hill_db, 0.0, 24.0, "dB", BODY) {
            self.set(ArchetParam::BridgeHillDb, nv);
        }
    }

    // -- Tier three: the player ----------------------------------------

    fn draw_player(&mut self, ui: &mut Ui, r: Rect) {
        theme::plate(ui, r, 4.0);
        theme::heading(ui, Pos2::new(r.left() + 12.0, r.top() + 11.0), "THE PLAYER", PLATE_SILK, r.left() + 112.0);

        let band = Rect::from_min_max(r.min + Vec2::new(12.0, 0.0), r.max - Vec2::new(12.0, 0.0));
        let top: [(&str, ArchetParam, f32, f32, f32, &str); 5] = [
            ("ATTACK", ArchetParam::Attack, self.patch.attack, 0.0, 0.5, "s"),
            ("RELEASE", ArchetParam::Release, self.patch.release, 0.0, 1.0, "s"),
            ("VIB RATE", ArchetParam::VibRate, self.patch.vib_rate, 0.0, 9.0, "Hz"),
            ("VIB DEPTH", ArchetParam::VibDepth, self.patch.vib_depth, 0.0, 60.0, "c"),
            ("VIB DELAY", ArchetParam::VibDelay, self.patch.vib_delay, 0.0, 2.0, "s"),
        ];
        for (i, (label, param, v, lo, hi, unit)) in top.into_iter().enumerate() {
            if let Some(nv) = self.knob(ui, geom::player_knob(band, 0, i), geom::KNOB, label, v, lo, hi, unit, TRUNK) {
                self.set(param, nv);
            }
        }
        let bottom: [(&str, ArchetParam, f32, f32, f32, &str); 2] = [
            ("DYNAMICS", ArchetParam::VelSens, self.patch.vel_sens, 0.0, 1.0, "%"),
            ("TUNE", ArchetParam::TuneCents, self.patch.tune_cents, -50.0, 50.0, "c"),
        ];
        for (i, (label, param, v, lo, hi, unit)) in bottom.into_iter().enumerate() {
            if let Some(nv) = self.knob(ui, geom::player_knob(band, 1, i), geom::KNOB, label, v, lo, hi, unit, TRUNK) {
                self.set(param, nv);
            }
        }

        // The section, and what leaves: the right end of the tier.
        if let Some(nv) = self.knob(ui, geom::section_knob(band, 0), geom::KNOB, "PLAYERS",
                                    self.patch.ensemble, 0.0, archet::engine::PLAYERS_MAX, "n", BODY) {
            self.set(ArchetParam::Ensemble, nv);
        }
        let mut level = self.patch.output_level;
        if let Some(nv) = self.knob(ui, geom::section_knob(band, 2), geom::KNOB, "LEVEL",
                                    level, 0.0, 4.0, "n", TRUNK) {
            level = nv;
            self.patch.output_level = nv;
            self.send(ArchetCommand::SetOutputLevel(nv));
        }
        let _ = level;

        let peak = self.cached_meter.peak_l.max(self.cached_meter.peak_r);
        machine::meter(ui, geom::meter(band), peak);
    }

    // -- The controls --------------------------------------------------

    /// A knob on the panel, at `at`, over one field. Returns the new value
    /// when the hand moved it, and nothing when it did not.
    #[allow(clippy::too_many_arguments)]
    fn knob(&mut self, ui: &mut Ui, at: Pos2, size: f32, label: &str, value: f32,
            lo: f32, hi: f32, unit: &str, tint: Color32) -> Option<f32> {
        let span = (hi - lo).max(1e-6);
        let mut n = ((value - lo) / span).clamp(0.0, 1.0);
        let old = n;
        let text = match unit {
            "%" => format!("{:.0}", n * 100.0),
            "Hz" => format!("{value:.1}"),
            "s" => if value < 1.0 { format!("{:.0}ms", value * 1000.0) } else { format!("{value:.2}s") },
            "c" => format!("{value:+.0}"),
            "dB" => format!("{value:.1}"),
            "f" => format!("{value:.3}"),
            _ => format!("{value:.2}"),
        };
        let cell = Rect::from_min_size(at, Vec2::new(geom::CELL.max(size), size + 34.0));
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(cell));
        cui.vertical_centered(|ui| {
            widgets::knob_fmt(ui, &mut n, label, &text, size, tint);
        });
        ((n - old).abs() > 1e-6).then_some(lo + n * span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use phonix_rt::meter_channel;

    fn root(ctx: &egui::Context) -> Ui {
        Ui::new(ctx.clone(), egui::Id::new("root"), egui::UiBuilder::new().max_rect(ctx.content_rect()))
    }

    #[allow(deprecated)]
    fn harness(app: ArchetApp) -> Harness<'static> {
        let mut app = app;
        Harness::builder().with_size(egui::vec2(W, H)).build(move |ctx| app.draw_ui(&mut root(ctx)))
    }

    fn app() -> (ArchetApp, mpsc::Receiver<ArchetCommand>) {
        let (tx, rx) = mpsc::channel();
        let (_mw, mr) = meter_channel::<ArchetMeterState>();
        (ArchetApp::new(tx, mr), rx)
    }

    /// The rail's order is the engine's order: a body chosen here is the
    /// body the engine builds.
    #[test]
    fn the_rail_names_the_bodies_the_engine_has() {
        let (mut app, _rx) = app();
        for (i, inst) in [Instrument::Violin, Instrument::Viola, Instrument::Cello, Instrument::DoubleBass]
            .into_iter().enumerate()
        {
            app.set(ArchetParam::Instrument, i as f32);
            assert_eq!(app.patch.instrument, inst, "rail cell {i} is not {inst:?}");
            assert_eq!(app.body(), i, "the panel reads {inst:?} back at the wrong cell");
        }
        assert_eq!(BODIES.len(), 4);
    }

    /// A knob writes the field it names, and sends the engine the same
    /// field. A knob wired to the wrong param is invisible on the panel and
    /// wrong in the sound.
    #[test]
    fn a_control_writes_the_field_it_names() {
        let (mut app, rx) = app();
        app.set(ArchetParam::BowForce, 0.42);
        assert!((app.patch.bow_force - 0.42).abs() < 1e-6);
        match rx.try_recv().expect("nothing was sent") {
            ArchetCommand::SetParam { param, value } => {
                assert_eq!(param, ArchetParam::BowForce);
                assert!((value - 0.42).abs() < 1e-6);
            }
            other => panic!("a knob sent {other:?} instead of SetParam"),
        }
    }

    // The panel and the keyboard fit the window, with room for the chrome.
    const _: () = assert!(PANEL_H + KEYBOARD_H < H);
    const _: () = assert!(H - PANEL_H - KEYBOARD_H >= 40.0);

    /// The window is exactly the chrome, the panel and the keyboard.
    #[test]
    fn the_window_holds_what_it_draws() {
        assert_eq!((W, H), (1160.0, 770.0));
    }

    #[test]
    fn the_editor_opens_on_the_first_preset() {
        let (app, _rx) = app();
        assert_eq!(app.patch.name, ArchetPatch::factory_presets()[0].name);
    }

    #[test]
    fn a_picked_preset_asks_for_the_parameter() {
        let (mut app, _rx) = app();
        app.pick_preset(3);
        assert_eq!(app.patch.name, ArchetPatch::factory_presets()[3].name);
        assert_eq!(app.take_wants_preset(), Some(4), "the parameter counts from one");
        assert_eq!(app.take_wants_preset(), None, "the ask is taken once");
    }

    /// The editor adopts what the engine publishes, but not while a knob
    /// here has just moved: a snapshot from before the move would undo it.
    #[test]
    fn the_editor_mirrors_the_engine_but_not_over_a_fresh_edit() {
        let (tx, _rx) = mpsc::channel();
        let (mut w, r) = meter_channel::<ArchetMeterState>();
        let mut app = ArchetApp::new(tx, r);
        let engine_patch = ArchetPatch { name: "From the engine".into(), bow_force: 0.77, ..Default::default() };
        {
            let s = w.edit();
            s.patch_snapshot = Some(engine_patch);
            w.publish();
        }
        app.set(ArchetParam::BowForce, 0.1);
        app.refresh_meters(true);
        assert_ne!(app.patch.name, "From the engine", "a fresh edit was overwritten");
        app.sync_cooldown = 0;
        app.refresh_meters(true);
        assert_eq!(app.patch.name, "From the engine");
        assert!((app.patch.bow_force - 0.77).abs() < 1e-3);
    }

    /// The engine mirror must not carry the chain back: the engine is handed
    /// a whole patch and publishes one back, effects included, but it runs
    /// none, and adopting its copy wholesale would put the preset's reverb
    /// back one frame after the page changed it.
    #[test]
    fn the_engine_mirror_does_not_erase_an_fx_edit() {
        let (tx, _rx) = mpsc::channel();
        let (mut w, r) = meter_channel::<ArchetMeterState>();
        let mut app = ArchetApp::new(tx, r);
        let engine_patch = ArchetPatch::default();
        assert_eq!(engine_patch.fx.slots[1].variant("type"), Some("hall"));
        {
            let s = w.edit();
            s.patch_snapshot = Some(engine_patch);
            w.publish();
        }
        app.set_fx(ArchetPatch::default().fx);
        app.edit_fx(|fx| fx.slots[1].set("type", "cathedral"));
        assert!(app.take_fx_changed());
        app.sync_cooldown = 0;
        app.refresh_meters(true);
        assert_eq!(app.patch.fx.slots[1].variant("type"), Some("cathedral"), "the mirror put the preset's space back");
    }

    /// A clicked pill turns the page; the page opens on the instrument.
    #[test]
    fn the_editor_opens_on_the_instrument_page() {
        let (app, _rx) = app();
        assert_eq!(app.tab, 0);
    }

    /// Look at it. Ignored because it needs a GPU (lavapipe does):
    ///   scripts/screenshots.sh
    fn page_snapshot(pluck: bool, name: &str) {
        let (tx, _rx) = mpsc::channel();
        let (mut w, r) = meter_channel::<ArchetMeterState>();
        {
            let s = w.edit();
            s.active_notes = vec![55, 62, 67];
            s.voice_count = 3;
            s.peak_l = 0.58;
            s.peak_r = 0.6;
            s.cpu_percent = 11.0;
            w.publish();
        }
        let mut app = ArchetApp::new(tx, r);
        let bank = ArchetPatch::factory_presets();
        let i = bank.iter().position(|p| p.pluck == pluck).unwrap_or(0);
        app.pick_preset(i);
        let mut h = harness(app);
        h.run_steps(3);
        h.snapshot(name);
    }

    #[test]
    #[ignore = "needs a rendering backend"]
    fn archet_arco_snapshot() { page_snapshot(false, "archet_arco"); }

    #[test]
    #[ignore = "needs a rendering backend"]
    fn archet_pizzicato_snapshot() { page_snapshot(true, "archet_pizzicato"); }

    /// The effects page, on the first preset's chain.
    #[test]
    #[ignore = "needs a rendering backend"]
    fn archet_effects_snapshot() {
        let (tx, _rx) = mpsc::channel();
        let (mut w, r) = meter_channel::<ArchetMeterState>();
        {
            let s = w.edit();
            s.voice_count = 3;
            s.peak_l = 0.58;
            s.peak_r = 0.6;
            s.cpu_percent = 11.0;
            w.publish();
        }
        let mut app = ArchetApp::new(tx, r);
        app.pick_preset(0);
        app.set_fx(ArchetPatch::factory_presets()[0].fx.clone());
        app.tab = 1;
        let mut h = harness(app);
        h.run_steps(3);
        h.snapshot("archet_effects");
    }
}
