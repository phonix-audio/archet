//! One effect's parameters, drawn from its `EffectSpec`.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use phonix_fx::{Curve, EffectSpec, ParamFlags, ParamKind, ParamSpec, SlotSpec, SpecValue, Value};

use super::RackStyle;
use crate::theme::{lamp, Palette};
use crate::widgets;

/// A slot's written parameters read against what its kind declares: an id
/// the slot never wrote reads as the kind's default, and writing it adds it.
pub struct SlotAccess<'a> {
    slot: &'a mut SlotSpec,
    spec: &'static EffectSpec,
    touched: bool,
}

impl<'a> SlotAccess<'a> {
    pub fn new(slot: &'a mut SlotSpec, spec: &'static EffectSpec) -> Self {
        SlotAccess { slot, spec, touched: false }
    }

    pub fn spec(&self) -> &'static EffectSpec {
        self.spec
    }

    pub fn slot(&mut self) -> &mut SlotSpec {
        self.slot
    }

    /// Whether a write changed anything since construction.
    pub fn touched(&self) -> bool {
        self.touched
    }

    pub fn value(&self, p: &ParamSpec) -> Value {
        match self.slot.get(p.id) {
            Some(v) => v.resolve(p).0,
            None => p.default,
        }
    }

    pub fn set(&mut self, p: &ParamSpec, value: impl Into<SpecValue>) {
        let value = value.into();
        if self.slot.get(p.id) != Some(&value) {
            self.slot.set(p.id, value);
            self.touched = true;
        }
    }
}

/// What a panel keeps between frames. Nothing yet: every control reads the
/// slot and writes it back. Kept as a type so an editor's storage does not
/// change when one of them needs state again.
#[derive(Default)]
pub struct PanelState {
    _private: (),
}

/// A parameter's cell: one knob wide for a number or a switch, two for a
/// list, which needs room for its longest label.
const CELL_W: f32 = widgets::KNOB_GROUP_W;
const LIST_W: f32 = widgets::KNOB_GROUP_W * 2.0 + 4.0;

fn to_norm(p: &ParamSpec, v: f32) -> f32 {
    match p.kind {
        ParamKind::Float { min, max, curve } => match curve {
            Curve::Log if min > 0.0 => ((v / min).ln() / (max / min).ln()).clamp(0.0, 1.0),
            Curve::Pow(k) => ((v - min) / (max - min)).clamp(0.0, 1.0).powf(1.0 / k),
            _ => ((v - min) / (max - min)).clamp(0.0, 1.0),
        },
        ParamKind::Int { min, max } => ((v - min as f32) / (max - min).max(1) as f32).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

fn from_norm(p: &ParamSpec, n: f32) -> f32 {
    match p.kind {
        ParamKind::Float { min, max, curve } => match curve {
            Curve::Log if min > 0.0 => min * (max / min).powf(n),
            Curve::Pow(k) => min + n.powf(k) * (max - min),
            _ => min + n * (max - min),
        },
        ParamKind::Int { min, max } => (min as f32 + n * (max - min) as f32).round(),
        _ => 0.0,
    }
}

/// The mix knob every slot has, beside its parameters.
fn mix_knob(ui: &mut Ui, slot: &mut SlotSpec, style: &RackStyle) -> bool {
    let mut norm = slot.mix.clamp(0.0, 1.0);
    let old = norm;
    widgets::knob_cell_for(ui, widgets::KNOB_GROUP_W, style.knob, |ui| {
        widgets::knob_fmt(ui, &mut norm, "Mix", &format!("{:.0}%", slot.mix * 100.0), style.knob, style.accent);
    });
    if (norm - old).abs() > 1e-6 {
        slot.mix = norm;
        return true;
    }
    false
}

/// Draw every parameter the kind declares, in the order it declares them,
/// wrapped in rows. Returns true when the mix moved; parameter writes are
/// reported by the access.
/// A parameter's label, under its control, where a knob's label sits.
fn cell_label(ui: &Ui, rect: Rect, text: &str, tint: Color32) {
    ui.painter().text(
        Pos2::new(rect.center().x, rect.bottom() - 12.0),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(9.0),
        tint,
    );
}

/// A list of named variants, in a cell of the panel's grid: the value on a
/// recessed strip that opens the list, the parameter's name under it.
fn list_cell(ui: &mut Ui, height: f32, label: &str, labels: &[&str], selected: usize, style: &RackStyle, salt: &str) -> Option<usize> {
    let pal = Palette::of(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(LIST_W, height), Sense::hover());
    // The combo's own node carries the value; the cell carries the
    // parameter's name, which is what a test and a screen reader look for.
    let shown = labels.get(selected).copied().unwrap_or("").to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, format!("{label}: {shown}")));
    let strip = Rect::from_min_size(rect.min + Vec2::new(0.0, 4.0), Vec2::new(LIST_W, 20.0));
    let mut out = None;
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(strip));
    let v = &mut cui.style_mut().visuals.widgets;
    for w in [&mut v.inactive, &mut v.hovered, &mut v.active, &mut v.open] {
        w.bg_fill = pal.well;
        w.weak_bg_fill = pal.well;
        w.bg_stroke = Stroke::new(1.0_f32, pal.well_edge);
    }
    cui.style_mut().spacing.button_padding = Vec2::new(4.0, 1.0);
    egui::ComboBox::from_id_salt(salt)
        .width(LIST_W - 4.0)
        .selected_text(egui::RichText::new(labels.get(selected).copied().unwrap_or("")).size(9.5).color(style.accent))
        .show_ui(&mut cui, |ui| {
            for (i, l) in labels.iter().enumerate() {
                if ui.selectable_label(i == selected, egui::RichText::new(*l).size(10.0)).clicked() && i != selected {
                    out = Some(i);
                }
            }
        });
    cell_label(ui, rect, label, pal.text_dim);
    out
}

/// Draw every parameter the kind declares, in the order it declares them,
/// each in a cell of one grid so a row wraps instead of piling up. Returns
/// true when the mix moved; parameter writes are reported by the access.
pub fn draw_panel(ui: &mut Ui, access: &mut SlotAccess, style: &RackStyle, _state: &mut PanelState, slot_index: usize) -> bool {
    let spec = access.spec();
    let pal = Palette::of(ui.ctx());
    let height = style.knob + widgets::GAP_KNOB_LABEL + 14.0;
    let mut mix_moved = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 6.0);
        for p in spec.params.iter() {
            if p.flags.contains(ParamFlags::READ_ONLY) {
                continue;
            }
            match p.kind {
                ParamKind::Float { .. } | ParamKind::Int { .. } => {
                    let value = access.value(p).as_f32();
                    let mut norm = to_norm(p, value);
                    let old = norm;
                    let text = match p.kind {
                        ParamKind::Int { .. } => format!("{}", value as i32),
                        _ => p.unit.format(value),
                    };
                    widgets::knob_cell_for(ui, CELL_W, style.knob, |ui| {
                        widgets::knob_fmt(ui, &mut norm, p.short, &text, style.knob, style.accent);
                    });
                    if (norm - old).abs() > 1e-6 {
                        let v = from_norm(p, norm);
                        match p.kind {
                            ParamKind::Int { .. } => access.set(p, v as i64),
                            _ => access.set(p, v),
                        }
                    }
                }
                ParamKind::Bool => {
                    let on = access.value(p).as_bool();
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(CELL_W, height), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Checkbox, true, p.short));
                    lamp(ui, Pos2::new(rect.center().x, rect.top() + style.knob * 0.5), 5.0, on, style.accent);
                    cell_label(ui, rect, p.short, if on { style.accent } else { pal.text_dim });
                    if resp.clicked() {
                        access.set(p, !on);
                    }
                }
                ParamKind::Enum { variants, labels } => {
                    let current = access.value(p).as_variant().unwrap_or("");
                    let idx = variants.iter().position(|v| *v == current).unwrap_or(0);
                    let salt = format!("{}_{}_{}", style.salt, slot_index, p.id);
                    if let Some(i) = list_cell(ui, height, p.short, labels, idx, style, &salt) {
                        if let Some(v) = variants.get(i) {
                            access.set(p, *v);
                        }
                    }
                }
                ParamKind::Bits { .. } => {}
            }
        }
        mix_moved = mix_knob(ui, access.slot(), style);
    });
    mix_moved
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;
    use phonix_fx::Registry;

    /// Every float and int a kind declares is a knob the accessibility tree
    /// sees, for every kind this build ships.
    #[test]
    fn every_declared_number_is_a_knob() {
        for entry in Registry::builtin().entries() {
            let spec = entry.spec;
            let mut slot = SlotSpec::new(spec.kind);
            let mut state = PanelState::default();
            let mut harness = Harness::builder().with_size(egui::vec2(900.0, 400.0)).build_ui(|ui| {
                let mut access = SlotAccess::new(&mut slot, spec);
                let style = RackStyle { accent: egui::Color32::WHITE, dim: egui::Color32::GRAY, knob: 32.0, salt: "t" };
                draw_panel(ui, &mut access, &style, &mut state, 0);
            });
            harness.run_steps(2);
            for p in spec.params.iter().filter(|p| matches!(p.kind, ParamKind::Float { .. } | ParamKind::Int { .. })) {
                assert!(harness.query_by_label(p.short).is_some(), "{}: no knob for {:?}", spec.kind, p.id);
            }
            assert!(harness.query_by_label("Mix").is_some(), "{}: no mix knob", spec.kind);
        }
    }

    /// Every list a kind declares is reachable too, and the panel's cells
    /// fit the width they are given: nothing is drawn outside the row.
    #[test]
    fn every_declared_list_is_a_cell() {
        for entry in Registry::builtin().entries() {
            let spec = entry.spec;
            let mut slot = SlotSpec::new(spec.kind);
            let mut state = PanelState::default();
            let mut harness = Harness::builder().with_size(egui::vec2(900.0, 400.0)).build_ui(|ui| {
                let mut access = SlotAccess::new(&mut slot, spec);
                let style = RackStyle { accent: egui::Color32::WHITE, dim: egui::Color32::GRAY, knob: 32.0, salt: "t" };
                draw_panel(ui, &mut access, &style, &mut state, 0);
            });
            harness.run_steps(2);
            for p in spec.params.iter().filter(|p| matches!(p.kind, ParamKind::Enum { .. } | ParamKind::Bool)) {
                let found = harness.query_all_by_label(p.short).count() >= 1
                    || harness.get_all_by_label_contains(p.short).count() >= 1;
                assert!(found, "{}: no cell for {:?}", spec.kind, p.id);
            }
        }
    }

    /// Look at it: the widest panel this build ships, at the width an
    /// editor gives it. Ignored because it needs a GPU (lavapipe does):
    ///   VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
    ///   UPDATE_SNAPSHOTS=1 cargo test -p phonix-ui --features rack panel_snapshot -- --ignored
    #[test]
    #[ignore = "needs a rendering backend"]
    fn panel_snapshot() {
        let registry = Registry::builtin();
        let entry = registry.entries().iter().max_by_key(|e| e.spec.params.len()).expect("a kind");
        let spec = entry.spec;
        let mut slot = SlotSpec::new(spec.kind);
        let mut state = PanelState::default();
        let mut harness = Harness::builder().with_size(egui::vec2(680.0, 180.0)).build_ui(move |ui| {
            let mut access = SlotAccess::new(&mut slot, spec);
            let style = RackStyle { accent: egui::Color32::from_rgb(255, 92, 208), dim: egui::Color32::GRAY, knob: 30.0, salt: "t" };
            draw_panel(ui, &mut access, &style, &mut state, 0);
        });
        harness.run_steps(3);
        harness.snapshot("rack_panel");
    }

    /// A value written by the panel lands in the slot under the spec's id,
    /// and an unwritten one reads as the kind's default.
    #[test]
    fn the_access_reads_defaults_and_writes_ids() {
        let reg = Registry::builtin();
        let entry = reg.get("delay").unwrap();
        let mut slot = SlotSpec::new("delay");
        let mut a = SlotAccess::new(&mut slot, entry.spec);
        let time = entry.spec.param("time").unwrap();
        assert_eq!(a.value(time), Value::F(0.3));
        a.set(time, 0.5_f32);
        assert!(a.touched());
        assert_eq!(slot.f32("time"), Some(0.5));
    }
}
