//! The effects rack: slots over a `ChainSpec`, each drawn from its kind's
//! `EffectSpec` and nothing else. A float is a knob with the unit the spec
//! names, a bool a lamp, an enum a row of pills or a list when there are
//! many, an int a knob that counts. Nothing here knows a kind by name.

mod panel;

pub use panel::{draw_panel, PanelState, SlotAccess};

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Ui, Vec2};
use phonix_fx::{ChainSpec, Registry, SlotSpec};

use crate::preset_picker::{picker_ui, NamedPreset, PresetPickerState, PresetPickerStyle};
use crate::theme::{lamp, Palette};
use crate::widgets;

/// How the rack is drawn: the instrument's accent and dim colours, the knob
/// size, and a salt that keeps two racks in one window apart.
pub struct RackStyle<'a> {
    pub accent: Color32,
    pub dim: Color32,
    pub knob: f32,
    pub salt: &'a str,
}

/// What the rack keeps between frames: one kind picker and one enum picker
/// set per slot.
#[derive(Default)]
pub struct RackState {
    kinds: Vec<PresetPickerState>,
    panels: Vec<panel::PanelState>,
}

impl RackState {
    fn slot(&mut self, i: usize) -> (&mut PresetPickerState, &mut panel::PanelState) {
        if self.kinds.len() <= i {
            self.kinds.resize_with(i + 1, Default::default);
            self.panels.resize_with(i + 1, Default::default);
        }
        (&mut self.kinds[i], &mut self.panels[i])
    }
}

const HEADER_H: f32 = 26.0;
const GAP: f32 = 8.0;

/// Draw `slots` slots of `spec`, offering the kinds `registry` holds. Slots
/// past `spec.len()` are empty and offer a kind. `peaks` are the chain's
/// per-slot peaks when the caller has them. Returns true when anything moved.
pub fn draw_rack(
    ui: &mut Ui,
    spec: &mut ChainSpec,
    slots: usize,
    registry: &Registry,
    peaks: Option<&[f32]>,
    style: &RackStyle,
    state: &mut RackState,
) -> bool {
    let mut changed = false;
    let kinds: Vec<&'static str> = registry.kinds().collect();
    let names: Vec<NamedPreset> = std::iter::once(NamedPreset { name: "None", category: None })
        .chain(registry.entries().iter().map(|e| NamedPreset { name: e.spec.name, category: None }))
        .collect();
    let pal = Palette::of(ui.ctx());

    for i in 0..slots {
        let (kind_picker, panel_state) = state.slot(i);
        let present = spec.slots.get(i).map(|s| s.kind.clone());
        kind_picker.current_idx = present.as_deref().and_then(|k| kinds.iter().position(|x| *x == k)).map_or(0, |p| p + 1);

        let width = ui.available_width();
        let (head, _) = ui.allocate_exact_size(Vec2::new(width, HEADER_H), Sense::hover());
        crate::theme::plate(ui, head, 3.0);
        let mut x = head.left() + 10.0;

        // The lamp is the slot's enable.
        let centre = Pos2::new(x, head.center().y);
        let enabled = spec.slots.get(i).map_or(false, |s| s.enabled);
        lamp(ui, centre, 4.0, enabled && present.is_some(), style.accent);
        let hit = Rect::from_center_size(centre, Vec2::splat(16.0));
        if ui.interact(hit, ui.id().with((style.salt, "on", i)), Sense::click()).clicked() {
            if let Some(s) = spec.slots.get_mut(i) {
                s.enabled = !s.enabled;
                changed = true;
            }
        }
        x += 16.0;

        // The kind, as a list.
        let strip = Rect::from_min_size(Pos2::new(x, head.top() + 2.0), Vec2::new(200.0, HEADER_H - 4.0));
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(strip));
        let salt = format!("{}_kind_{i}", style.salt);
        if let Some(idx) = picker_ui(&mut cui, &PresetPickerStyle { salt: &salt, accent: style.accent, dim: style.dim }, kind_picker, &names) {
            if idx == 0 {
                if i < spec.slots.len() {
                    spec.slots.remove(i);
                    changed = true;
                }
            } else {
                let kind = kinds[idx - 1];
                while spec.slots.len() < i {
                    spec.slots.push(SlotSpec::new("").enabled(false));
                }
                if i < spec.slots.len() {
                    if spec.slots[i].kind != kind {
                        spec.slots[i] = SlotSpec::new(kind);
                        changed = true;
                    }
                } else {
                    spec.slots.push(SlotSpec::new(kind));
                    changed = true;
                }
            }
        }
        x += 210.0;

        // The slot's number, dim, and its peak on the right.
        ui.painter().text(Pos2::new(head.right() - 76.0, head.center().y), Align2::RIGHT_CENTER, format!("{}", i + 1), FontId::monospace(9.0), pal.text_dim);
        if let Some(p) = peaks.and_then(|p| p.get(i)) {
            let meter = Rect::from_min_size(Pos2::new(head.right() - 70.0, head.center().y - 4.0), Vec2::new(60.0, 8.0));
            let mut mui = ui.new_child(egui::UiBuilder::new().max_rect(meter));
            widgets::peak_meter(&mut mui, *p, meter.size(), style.accent);
        }
        let _ = x;

        // The panel, when the slot holds a kind this build knows.
        if let Some(slot) = spec.slots.get_mut(i) {
            if slot.kind.is_empty() {
                ui.add_space(GAP);
                continue;
            }
            match registry.get(&slot.kind) {
                Some(entry) => {
                    ui.add_space(4.0);
                    let mut access = SlotAccess::new(slot, entry.spec);
                    changed |= draw_panel(ui, &mut access, style, panel_state, i);
                    changed |= access.touched();
                }
                None => {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!("{}: not in this build, kept as written", slot.kind)).color(pal.text_dim).size(10.0));
                }
            }
        }
        ui.add_space(GAP);
    }
    changed
}
