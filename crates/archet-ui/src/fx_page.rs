//! The effects page: the chain a patch carries, opened up.
//!
//! Which effects run and in what order is not editable here: that is what
//! curated means, and the recipe lives with the patch in `archet::fx`.
//! Everything inside each effect is. Drawn on the same plates as the
//! instrument page, and each effect shows what it does: the equaliser its
//! response, from the same numbers the audio
//! thread runs.

use egui::{Align2, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use phonix_fx::effects::parametric_eq::ParametricEq;
use phonix_fx::{ChainSpec, EffectSpec, ParamKind, ParamSpec, Registry, SlotSpec, SpecValue};
use phonix_ui::preset_picker::{picker_ui, NamedPreset, PresetPickerState, PresetPickerStyle};
use phonix_ui::theme::{engraved, fill_polygon};
use phonix_ui::widgets;

use crate::colors::*;
use crate::theme;

/// What the page keeps between frames: the dropdown's own state, and the
/// kinds this build draws.
pub struct FxPageState {
    reverb_picker: PresetPickerState,
    registry: Registry,
}

impl Default for FxPageState {
    fn default() -> Self {
        FxPageState { reverb_picker: PresetPickerState::default(), registry: Registry::builtin() }
    }
}

/// One slot's written parameters, read against what the kind declares: an
/// id the recipe never wrote reads as the kind's default, and writing it
/// adds it.
struct SlotAccess<'a> {
    slot: &'a mut SlotSpec,
    spec: &'static EffectSpec,
    touched: bool,
}

impl SlotAccess<'_> {
    fn param(&self, id: &str) -> Option<&ParamSpec> {
        self.spec.param(id)
    }

    /// The value the effect holds for `id`; zero for an id the kind does
    /// not declare, so a panel drawn for another version of the kind shows
    /// a still knob rather than nothing at all.
    fn get(&self, id: &str) -> f32 {
        let Some(p) = self.param(id) else { return 0.0 };
        match self.slot.get(id) {
            Some(v) => v.resolve(p).0.as_f32(),
            None => p.default.as_f32(),
        }
    }

    fn get_bool(&self, id: &str) -> bool {
        let Some(p) = self.param(id) else { return false };
        match self.slot.get(id) {
            Some(v) => v.resolve(p).0.as_bool(),
            None => p.default.as_bool(),
        }
    }

    fn variant(&self, id: &str) -> &'static str {
        let Some(p) = self.param(id) else { return "" };
        match self.slot.get(id) {
            Some(v) => v.resolve(p).0.as_variant().unwrap_or(""),
            None => p.default.as_variant().unwrap_or(""),
        }
    }

    fn set(&mut self, id: &str, value: impl Into<SpecValue>) {
        let value = value.into();
        if self.slot.get(id) != Some(&value) {
            self.slot.set(id, value);
            self.touched = true;
        }
    }
}

const GAP: f32 = 16.0;
const KNOB: f32 = 34.0;
const PITCH_X: f32 = 62.0;
const PITCH_Y: f32 = KNOB + 26.0 + 10.0;
const PLOT_H: f32 = 104.0;
/// The rate the response curves are drawn at. A picture, not the audio:
/// the shape of a band does not move between 44.1 and 96 kHz on a plot
/// that spans 20 Hz to 20 kHz.
const CURVE_SR: f32 = 48_000.0;
/// Vertical span of the EQ plot, in dB either side of unity.
const EQ_PLOT_DB: f32 = 18.0;

/// A knob over a declared float parameter: its range, curve and unit come
/// from the kind.
fn knob(ui: &mut Ui, at: Pos2, label: &str, access: &mut SlotAccess, id: &str) {
    let Some(p) = access.param(id).copied() else { return };
    let ParamKind::Float { min, max, curve } = p.kind else { return };
    let value = access.get(id);
    let to_norm = |v: f32| match curve {
        phonix_fx::Curve::Log if min > 0.0 => ((v / min).ln() / (max / min).ln()).clamp(0.0, 1.0),
        _ => ((v - min) / (max - min)).clamp(0.0, 1.0),
    };
    let from_norm = |n: f32| match curve {
        phonix_fx::Curve::Log if min > 0.0 => min * (max / min).powf(n),
        _ => min + n * (max - min),
    };
    let mut norm = to_norm(value);
    let old = norm;
    let cell = Rect::from_min_size(at, Vec2::new(widgets::KNOB_GROUP_W, KNOB + 26.0));
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(cell));
    cui.vertical_centered(|ui| {
        widgets::knob_fmt(ui, &mut norm, label, &p.unit.format(value), KNOB, ROSIN);
    });
    if (norm - old).abs() > 1e-6 {
        access.set(id, from_norm(norm));
    }
}

/// The slot's own mix, drawn like any knob.
fn mix_knob(ui: &mut Ui, at: Pos2, slot: &mut SlotSpec) -> bool {
    let mut norm = slot.mix.clamp(0.0, 1.0);
    let old = norm;
    let cell = Rect::from_min_size(at, Vec2::new(widgets::KNOB_GROUP_W, KNOB + 26.0));
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(cell));
    cui.vertical_centered(|ui| {
        widgets::knob_fmt(ui, &mut norm, "MIX", &format!("{:.0}%", slot.mix * 100.0), KNOB, ROSIN);
    });
    if (norm - old).abs() > 1e-6 {
        slot.mix = norm;
        return true;
    }
    false
}

/// A lamp with a word beside it, and the whole thing a switch.
fn lamp_switch(ui: &mut Ui, centre: Pos2, on: bool, label: &str, salt: impl std::hash::Hash) -> bool {
    theme::lamp(ui, centre, 4.0, on, ROSIN);
    theme::printed(ui, centre + Vec2::new(10.0, 0.0), label, FontId::proportional(8.5),
                   if on { PLATE_SILK } else { PLATE_SILK_DIM }, Align2::LEFT_CENTER);
    let hit = Rect::from_center_size(centre + Vec2::new(22.0, 0.0), Vec2::new(64.0, 18.0));
    ui.interact(hit, ui.id().with(salt), Sense::click()).clicked()
}

/// The dark well every plot sits in.
fn plot_frame(ui: &Ui, r: Rect) {
    ui.painter().rect_filled(r, 3.0, WELL);
    ui.painter().rect_stroke(r, 3.0, Stroke::new(1.0_f32, WELL_EDGE), StrokeKind::Inside);
}

fn caption(ui: &Ui, at: Pos2, text: &str) {
    theme::printed(ui, at, text, FontId::proportional(8.5), PLATE_SILK_DIM, Align2::LEFT_TOP);
}

fn hz(v: f32) -> String {
    if v >= 1000.0 { format!("{:.1} kHz", v / 1000.0) } else { format!("{v:.0} Hz") }
}

/// The equaliser's response, from the effect's own filters.
fn eq_plot(ui: &Ui, r: Rect, a: &SlotAccess) {
    plot_frame(ui, r);
    let mut eq = ParametricEq::new(CURVE_SR);
    for b in 0..4 {
        eq.set_band_freq(b, a.get(&format!("band.{b}.freq")));
        eq.set_band_gain(b, a.get(&format!("band.{b}.gain")));
        eq.set_band_q(b, a.get(&format!("band.{b}.q")));
        eq.set_band_enabled(b, a.get_bool(&format!("band.{b}.enabled")));
        let type_id = format!("band.{b}.type");
        let t = a.param(&type_id).and_then(|p| p.variant_index(a.variant(&type_id))).unwrap_or(0);
        eq.set_band_type(b, t as u8);
    }

    // Log frequency across, dB up. Ticks where an ear counts: 100, 1k, 10k.
    let (f0, f1) = (20.0f32, 20_000.0f32);
    let x_of = |f: f32| r.left() + (f / f0).log10() / (f1 / f0).log10() * r.width();
    let y_of = |d: f32| r.center().y - (d / EQ_PLOT_DB).clamp(-1.0, 1.0) * (r.height() * 0.5 - 6.0);
    for f in [100.0, 1000.0, 10_000.0] {
        let x = x_of(f);
        ui.painter().line_segment([Pos2::new(x, r.top()), Pos2::new(x, r.bottom())],
                                  Stroke::new(1.0_f32, WELL_EDGE.gamma_multiply(0.6)));
        ui.painter().text(Pos2::new(x + 3.0, r.bottom() - 2.0), Align2::LEFT_BOTTOM, hz(f),
                          FontId::proportional(8.0), PLATE_SILK_DIM);
    }
    ui.painter().line_segment([Pos2::new(r.left(), y_of(0.0)), Pos2::new(r.right(), y_of(0.0))],
                              Stroke::new(1.0_f32, WELL_EDGE));

    let n = r.width() as usize;
    let mut line = Vec::with_capacity(n + 1);
    let mut area = Vec::with_capacity(n + 3);
    area.push(Pos2::new(r.left(), y_of(0.0)));
    for i in 0..=n {
        let f = f0 * (f1 / f0).powf(i as f32 / n as f32);
        let d = 20.0 * eq.magnitude_at(f).max(1e-6).log10();
        let p = Pos2::new(x_of(f), y_of(d));
        line.push(p);
        area.push(p);
    }
    area.push(Pos2::new(r.right(), y_of(0.0)));
    fill_polygon(ui, &area, ROSIN.gamma_multiply(0.18), ROSIN.gamma_multiply(0.06));
    ui.painter().add(Shape::line(line, Stroke::new(1.5_f32, ROSIN)));
}

/// What each band is under its automatic type, which the recipe never changes.
const BAND_NAMES: [&str; 4] = ["LOW\nSHELF", "PEAK 1", "PEAK 2", "HIGH\nSHELF"];

/// Draw the page. Returns true when anything was moved, so the caller knows
/// to publish the chain.
pub fn draw(ui: &mut Ui, r: Rect, spec: &mut ChainSpec, state: &mut FxPageState) -> bool {
    if spec.is_empty() {
        engraved(ui, r.center(), "this patch carries no effects", FontId::proportional(12.0), SILK_DIM,
                 Align2::CENTER_CENTER);
        return false;
    }

    let mut changed = false;
    let count = spec.len();
    let col_w = (r.width() - GAP * (count as f32 - 1.0)) / count as f32;

    for (i, slot) in spec.slots.iter_mut().enumerate() {
        let col = Rect::from_min_size(
            Pos2::new(r.left() + i as f32 * (col_w + GAP), r.top()),
            Vec2::new(col_w, r.height()),
        );
        theme::plate(ui, col, 4.0);
        let Some(entry) = state.registry.get(&slot.kind) else {
            theme::heading(ui, Pos2::new(col.left() + 12.0, col.top() + 11.0), &slot.kind.to_uppercase(),
                           PLATE_SILK, col.left() + 120.0);
            caption(ui, Pos2::new(col.left() + 12.0, col.top() + 30.0), "not in this build; kept as written");
            continue;
        };
        let espec = entry.spec;
        let x0 = col.left() + 12.0;
        let inner_w = col.width() - 24.0;
        let mut y = col.top() + 11.0;

        theme::heading(ui, Pos2::new(x0, y), &espec.name.to_uppercase(), PLATE_SILK, x0 + 120.0);
        if lamp_switch(ui, Pos2::new(col.right() - 56.0, y), slot.enabled,
                       if slot.enabled { "ON" } else { "OFF" }, ("fx_on", i)) {
            slot.enabled = !slot.enabled;
            changed = true;
        }
        y += 22.0;

        let plot = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(inner_w, PLOT_H));
        let mut a = SlotAccess { slot, spec: espec, touched: false };

        match espec.kind {
            "parametric-eq" => {
                eq_plot(ui, plot, &a);
                y += PLOT_H + 10.0;
                for (b, band_name) in BAND_NAMES.iter().enumerate() {
                    let row_y = y + b as f32 * PITCH_Y;
                    let on_id = format!("band.{b}.enabled");
                    let on = a.get_bool(&on_id);
                    let c = Pos2::new(x0 + 8.0, row_y + 14.0);
                    theme::lamp(ui, c, 4.0, on, ROSIN);
                    theme::printed(ui, Pos2::new(x0 + 2.0, row_y + 26.0), band_name, FontId::proportional(8.0),
                                   if on { PLATE_SILK } else { PLATE_SILK_DIM }, Align2::LEFT_TOP);
                    let hit = Rect::from_min_size(Pos2::new(x0, row_y), Vec2::new(48.0, KNOB + 26.0));
                    if ui.interact(hit, ui.id().with(("eq_band", b)), Sense::click()).clicked() {
                        a.set(&on_id, !on);
                    }
                    let at = |c: usize| Pos2::new(x0 + 50.0 + c as f32 * PITCH_X, row_y);
                    knob(ui, at(0), "FREQ", &mut a, &format!("band.{b}.freq"));
                    knob(ui, at(1), "GAIN", &mut a, &format!("band.{b}.gain"));
                    knob(ui, at(2), "Q", &mut a, &format!("band.{b}.q"));
                }
                y += 4.0 * PITCH_Y;
                caption(ui, Pos2::new(x0, y), "flat by design: the bodies are calibrated on\nrecordings; the bands are yours");
            }
            "reverb" => {
                // The type is a choice, not a quantity: a list, not a dial.
                let (variants, labels): (&[&str], &[&str]) = match a.param("type").map(|p| p.kind) {
                    Some(ParamKind::Enum { variants, labels }) => (variants, labels),
                    _ => (&[], &[]),
                };
                let names: Vec<NamedPreset> = labels.iter().map(|l| NamedPreset { name: l, category: None }).collect();
                let idx = variants.iter().position(|v| *v == a.variant("type")).unwrap_or(0);
                state.reverb_picker.current_idx = idx;
                let strip = Rect::from_min_size(Pos2::new(x0, y), Vec2::new(inner_w, 26.0));
                let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(strip));
                if let Some(new_idx) = picker_ui(
                    &mut cui,
                    &PresetPickerStyle { salt: "archet_reverb_type", accent: ROSIN, dim: SILK_DIM, name_in_combo: true },
                    &mut state.reverb_picker,
                    &names,
                ) {
                    if let Some(v) = variants.get(new_idx) {
                        a.set("type", *v);
                    }
                }
                y += 40.0;
                let at = |c: usize, row: usize| Pos2::new(x0 + c as f32 * PITCH_X, y + row as f32 * PITCH_Y);
                knob(ui, at(0, 0), "SIZE", &mut a, "size");
                knob(ui, at(1, 0), "DECAY", &mut a, "decay");
                knob(ui, at(2, 0), "DAMP", &mut a, "damping");
                knob(ui, at(3, 0), "WIDTH", &mut a, "width");
                knob(ui, at(0, 1), "PRE", &mut a, "predelay");
                changed |= mix_knob(ui, at(1, 1), a.slot);
                y += 2.0 * PITCH_Y;
                caption(ui, Pos2::new(x0, y), "the space the string radiates into:\nthe model has no walls of its own");
            }
            "stereo-imager" => {
                let at = |c: usize| Pos2::new(x0 + c as f32 * PITCH_X, y);
                knob(ui, at(0), "WIDTH", &mut a, "width");
                knob(ui, at(1), "MONO", &mut a, "mono-freq");
                changed |= mix_knob(ui, at(2), a.slot);
                y += PITCH_Y;
                caption(ui, Pos2::new(x0, y), "where the listener sits: a soloist close\nand narrow, a section across the stage");
            }
            _ => {
                // A kind without a panel of its own: every float it declares, in rows of four.
                let ids: Vec<&str> = espec.params.iter()
                    .filter(|p| matches!(p.kind, ParamKind::Float { .. })).map(|p| p.id).collect();
                for (n, id) in ids.iter().enumerate() {
                    let at = Pos2::new(x0 + (n % 4) as f32 * PITCH_X, y + (n / 4) as f32 * PITCH_Y);
                    knob(ui, at, espec.param(id).map_or(id, |p| p.short), &mut a, id);
                }
            }
        }
        changed |= a.touched;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_reads_defaults_and_writes_only_what_moved() {
        let mut slot = SlotSpec::new("reverb").with("size", 0.35_f32).mix(0.22);
        let spec = Registry::builtin().get("reverb").unwrap().spec;
        let mut a = SlotAccess { slot: &mut slot, spec, touched: false };
        assert_eq!(a.get("size"), 0.35);
        assert_eq!(a.get("decay"), 0.5, "the kind's default, not zero");
        assert_eq!(a.variant("type"), "hall");
        a.set("size", 0.35_f32);
        assert!(!a.touched, "writing the same value is not an edit");
        a.set("size", 0.5_f32);
        assert!(a.touched);
        a.set("type", "cathedral");
        assert_eq!(a.variant("type"), "cathedral");
    }

    #[test]
    fn every_recipe_slot_has_a_panel_in_this_build() {
        let reg = Registry::builtin();
        for slot in archet::fx::chain(archet::fx::CHAMBER).slots {
            assert!(reg.contains(&slot.kind), "{} is not compiled in", slot.kind);
        }
    }

    #[test]
    fn a_knob_over_a_declared_parameter_formats_with_its_unit() {
        let spec = Registry::builtin().get("reverb").unwrap().spec;
        assert_eq!(spec.param("predelay").unwrap().unit.format(0.02), "20 ms");
    }
}
