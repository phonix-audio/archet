//! The painted controls every editor shares: a switch cut into the plate,
//! a raised button, a button that stays with its lamp, a pad, a stepper, a
//! fader's cap and an LED ladder.
//!
//! A control takes the rect it is drawn in; the `flow_` forms allocate that
//! rect in the layout the caller is in, sized to their text. Each names
//! itself to the accessibility tree with its label, so a test finds it.
//! Every one draws with `theme` and reads the installed `Palette`.

use egui::{Align2, Color32, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, WidgetInfo, WidgetType};

use crate::theme::{cast_shadow, gradient_rounded, lighten, lit_edges, printed, radial_disc, recess, Palette};

/// The corner of a button, a key of a switch and a pad.
pub const RADIUS: f32 = 3.0;
/// The height of a flowed control.
pub const FLOW_H: f32 = 22.0;

fn name(r: &Response, label: &str, on: bool) {
    let label = label.to_string();
    r.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, on, label.clone()));
}

/// The face of a raised part: its shadow, a gradient lit from above, the
/// light on its top edge and its rim. `lit` fills it with the tint; `down`
/// presses it into the plate, one pixel lower and without its shadow.
pub fn face(ui: &Ui, rect: Rect, lit: bool, down: bool, hovered: bool, tint: Color32) {
    let pal = Palette::of(ui.ctx());
    let r = if down { rect.translate(Vec2::new(0.0, 1.0)) } else { rect };
    if !down {
        cast_shadow(ui, r, RADIUS as u8, 1.0);
    }
    let (top, bottom) = if lit {
        (tint.gamma_multiply(0.55), tint.gamma_multiply(0.28))
    } else {
        (lighten(pal.bg_raised, 0.07), pal.bg_panel)
    };
    let (top, bottom) = if down { (bottom, top) } else { (top, bottom) };
    gradient_rounded(ui, r, RADIUS as u8, top, bottom);
    if !down {
        lit_edges(ui, r, RADIUS);
    }
    let rim = if lit { tint } else if hovered { pal.silk_dim.gamma_multiply(0.8) } else { pal.shade_edge };
    ui.painter().rect_stroke(r, RADIUS, Stroke::new(1.0_f32, rim), StrokeKind::Inside);
}

/// A row of exclusive choices in a slot cut into the plate: the chosen one
/// is a key pressed in and lit.
pub fn switch(ui: &mut Ui, rect: Rect, labels: &[&str], sel: usize, tint: Color32, salt: impl std::hash::Hash) -> Option<usize> {
    let w = rect.width() / labels.len().max(1) as f32;
    let widths = vec![w; labels.len()];
    switch_keys(ui, rect, labels, &widths, sel, tint, salt)
}

/// `switch` with a width of its own for each key.
fn switch_keys(ui: &mut Ui, rect: Rect, labels: &[&str], widths: &[f32], sel: usize, tint: Color32, salt: impl std::hash::Hash) -> Option<usize> {
    let pal = Palette::of(ui.ctx());
    recess(ui, rect, RADIUS);
    let mut picked = None;
    let id = ui.id().with(salt);
    let font = FontId::proportional(pal.type_silk);
    let mut x = rect.left();
    for (i, label) in labels.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(0.0);
        let cell = Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w, rect.height()));
        x += w;
        let r = ui.interact(cell, id.with(i), Sense::click());
        name(&r, label, i == sel);
        if r.clicked() && i != sel {
            picked = Some(i);
        }
        let key = cell.shrink(2.0);
        if i == sel {
            gradient_rounded(ui, key, 2, tint.gamma_multiply(0.42), tint.gamma_multiply(0.22));
            ui.painter().rect_stroke(key, 2.0, Stroke::new(1.0_f32, tint), StrokeKind::Inside);
        } else if r.hovered() {
            ui.painter().rect_stroke(key, 2.0, Stroke::new(1.0_f32, pal.silk_dim.gamma_multiply(0.5)), StrokeKind::Inside);
        }
        if i > 0 && i != sel && i != sel + 1 {
            let x = cell.left();
            ui.painter().line_segment([Pos2::new(x, rect.top() + 4.0), Pos2::new(x, rect.bottom() - 4.0)], Stroke::new(1.0_f32, pal.well_edge));
        }
        let ink = if i == sel { pal.silk } else { pal.silk_dim };
        printed(ui, cell.center(), label, font.clone(), ink, Align2::CENTER_CENTER);
    }
    picked
}

/// A raised button, lit in the tint while `lit`.
pub fn button(ui: &mut Ui, rect: Rect, label: &str, lit: bool, tint: Color32, salt: impl std::hash::Hash) -> Response {
    let pal = Palette::of(ui.ctx());
    let r = ui.interact(rect, ui.id().with(salt), Sense::click_and_drag());
    name(&r, label, lit);
    let down = r.is_pointer_button_down_on();
    face(ui, rect, lit, down, r.hovered(), tint);
    let at = rect.center() + if down { Vec2::new(0.0, 1.0) } else { Vec2::ZERO };
    printed(ui, at, label, FontId::proportional(pal.type_label), if lit || r.hovered() { pal.silk } else { pal.silk_dim }, Align2::CENTER_CENTER);
    r
}

/// A button that stays: a lamp at its left says whether it is on.
pub fn lamp_button(ui: &mut Ui, rect: Rect, label: &str, on: bool, tint: Color32, salt: impl std::hash::Hash) -> Response {
    let pal = Palette::of(ui.ctx());
    let r = ui.interact(rect, ui.id().with(salt), Sense::click());
    name(&r, label, on);
    let down = r.is_pointer_button_down_on();
    face(ui, rect, false, down, r.hovered(), tint);
    let dy = if down { 1.0 } else { 0.0 };
    crate::theme::lamp(ui, Pos2::new(rect.left() + 9.0, rect.center().y + dy), 3.2, on, tint);
    let ink = if on { pal.silk } else { pal.silk_dim };
    printed(ui, Pos2::new(rect.left() + 18.0, rect.center().y + dy), label, FontId::proportional(pal.type_silk), ink, Align2::LEFT_CENTER);
    r
}

/// A pad: a rubber key sunk in a well, lit in its colour when set.
pub fn pad(ui: &mut Ui, rect: Rect, label: &str, lit: Option<Color32>, salt: impl std::hash::Hash) -> Response {
    let pal = Palette::of(ui.ctx());
    let r = ui.interact(rect, ui.id().with(salt), Sense::click());
    name(&r, label, lit.is_some());
    recess(ui, rect, RADIUS);
    let down = r.is_pointer_button_down_on();
    let key = rect.shrink(2.5).translate(Vec2::new(0.0, if down { 1.0 } else { 0.0 }));
    match lit {
        Some(c) => {
            ui.painter().rect_filled(key.expand(2.0), RADIUS + 1.0, c.gamma_multiply(0.18));
            gradient_rounded(ui, key, 2, lighten(c, 0.25), c.gamma_multiply(0.7));
        }
        None => {
            let base = lighten(pal.well, if r.hovered() { 0.08 } else { 0.04 });
            gradient_rounded(ui, key, 2, base, pal.well);
        }
    }
    ui.painter().line_segment(
        [Pos2::new(key.left() + 2.0, key.top() + 0.5), Pos2::new(key.right() - 2.0, key.top() + 0.5)],
        Stroke::new(1.0_f32, pal.light_edge),
    );
    let ink = if lit.is_some() { Color32::from_rgb(12, 12, 14) } else { pal.silk_dim };
    ui.painter().text(key.center(), Align2::CENTER_CENTER, label, FontId::proportional(pal.type_value), ink);
    r
}

/// A value in a well with a step either side of it: -1 or 1 when one was
/// clicked.
pub fn stepper(ui: &mut Ui, rect: Rect, text: &str, salt: impl std::hash::Hash) -> Option<i32> {
    let pal = Palette::of(ui.ctx());
    recess(ui, rect, RADIUS);
    let id = ui.id().with(salt);
    let w = 14.0;
    let left = Rect::from_min_size(rect.min, Vec2::new(w, rect.height()));
    let right = Rect::from_min_size(Pos2::new(rect.right() - w, rect.top()), Vec2::new(w, rect.height()));
    let rl = ui.interact(left, id.with(0), Sense::click());
    let rr = ui.interact(right, id.with(1), Sense::click());
    name(&rl, "STEP DOWN", false);
    name(&rr, "STEP UP", false);
    chevron(ui, left.center(), -1.0, if rl.hovered() { pal.silk } else { pal.silk_dim });
    chevron(ui, right.center(), 1.0, if rr.hovered() { pal.silk } else { pal.silk_dim });
    for x in [left.right(), right.left()] {
        ui.painter().line_segment([Pos2::new(x, rect.top() + 4.0), Pos2::new(x, rect.bottom() - 4.0)], Stroke::new(1.0_f32, pal.well_edge));
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, FontId::monospace(pal.type_silk), pal.silk);
    if rl.clicked() {
        Some(-1)
    } else if rr.clicked() {
        Some(1)
    } else {
        None
    }
}

/// A small chevron pointing left (`dir` < 0) or right.
pub fn chevron(ui: &Ui, at: Pos2, dir: f32, ink: Color32) {
    let d = dir.signum() * 2.2;
    ui.painter().line_segment([at + Vec2::new(-d, -3.0), at + Vec2::new(d, 0.0)], Stroke::new(1.3_f32, ink));
    ui.painter().line_segment([at + Vec2::new(d, 0.0), at + Vec2::new(-d, 3.0)], Stroke::new(1.3_f32, ink));
}

/// A fader's cap: a raised grip with the line it reads at across it,
/// vertical when the fader runs across.
pub fn cap(ui: &Ui, rect: Rect, tint: Color32, across: bool) {
    let pal = Palette::of(ui.ctx());
    cast_shadow(ui, rect, 2, 2.0);
    gradient_rounded(ui, rect, 2, lighten(pal.bg_raised, 0.22), pal.bg_panel);
    lit_edges(ui, rect, 2.0);
    ui.painter().rect_stroke(rect, 2.0, Stroke::new(1.0_f32, pal.shade_edge), StrokeKind::Inside);
    let c = rect.center();
    let line = if across {
        [Pos2::new(c.x, rect.top() + 2.0), Pos2::new(c.x, rect.bottom() - 2.0)]
    } else {
        [Pos2::new(rect.left() + 2.0, c.y), Pos2::new(rect.right() - 2.0, c.y)]
    };
    ui.painter().line_segment(line, Stroke::new(1.5_f32, tint));
}

/// The height of a fader's cap.
pub const CAP_H: f32 = 12.0;

/// A vertical fader: a slot with ticks beside it and a cap, 1 at the top
/// and 0 at the bottom. Returns the response and the value the hand moved
/// it to.
pub fn fader(ui: &mut Ui, slot: Rect, value: f32, tint: Color32, salt: impl std::hash::Hash) -> (Response, Option<f32>) {
    let pal = Palette::of(ui.ctx());
    let hit = slot.expand2(Vec2::new(10.0, 0.0));
    let r = ui.interact(hit, ui.id().with(salt), Sense::click_and_drag());
    r.widget_info(|| WidgetInfo::slider(true, value as f64, "FADER"));
    recess(ui, slot, RADIUS);
    let span = (slot.height() - CAP_H).max(1.0);
    for k in 0..=4 {
        let y = slot.top() + CAP_H * 0.5 + k as f32 / 4.0 * span;
        ui.painter().line_segment([Pos2::new(slot.left() - 4.0, y), Pos2::new(slot.left() - 1.0, y)], Stroke::new(1.0_f32, pal.silk_dim));
    }
    let mid = slot.center().x;
    ui.painter().line_segment([Pos2::new(mid, slot.top() + 4.0), Pos2::new(mid, slot.bottom() - 4.0)], Stroke::new(1.0_f32, pal.well_edge));
    let y = slot.top() + CAP_H * 0.5 + (1.0 - value.clamp(0.0, 1.0)) * span;
    cap(ui, Rect::from_center_size(Pos2::new(mid, y), Vec2::new(slot.width() + 8.0, CAP_H)), tint, false);
    let moved = if r.dragged() || r.clicked() {
        r.interact_pointer_pos()
            .map(|p| (1.0 - (p.y - slot.top() - CAP_H * 0.5) / span).clamp(0.0, 1.0))
            .filter(|v| (v - value).abs() > 1e-4)
    } else {
        None
    };
    (r, moved)
}

/// `fader` in the flow, `height` tall.
pub fn flow_fader(ui: &mut Ui, height: f32, value: f32, tint: Color32) -> (Response, Option<f32>) {
    let id = ui.next_auto_id();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(28.0, height), Sense::hover());
    let slot = Rect::from_center_size(rect.center(), Vec2::new(10.0, height));
    fader(ui, slot, value, tint, id)
}

/// The colour a ladder's segment at `t` of full scale lights in.
pub fn ladder_colour(pal: &Palette, t: f32) -> Color32 {
    if t >= 0.9 {
        pal.warm
    } else if t >= 0.7 {
        pal.warn
    } else {
        pal.ok
    }
}

/// An LED ladder in a well: segments lit up to `level` of full scale, dark
/// ones still showing their colour, and the highest level of the last
/// second held one segment lit.
pub fn ladder(ui: &Ui, rect: Rect, level: f32, vertical: bool) {
    let pal = Palette::of(ui.ctx());
    recess(ui, rect, 2.0);
    let inner = rect.shrink(2.0);
    let long = if vertical { inner.height() } else { inner.width() };
    if long < 4.0 {
        return;
    }
    let pitch = 3.0;
    let n = (long / pitch).floor().max(1.0) as usize;
    let level = level.clamp(0.0, 1.0);
    let id = egui::Id::new(("phonix ladder", (rect.min.x * 4.0) as i32, (rect.min.y * 4.0) as i32));
    let now = ui.input(|i| i.time);
    let (mut hold, mut since) = ui.ctx().data(|d| d.get_temp::<(f32, f64)>(id)).unwrap_or((0.0, now));
    if level >= hold || now - since > 1.0 {
        hold = level;
        since = now;
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, (hold, since)));
    let held = ((hold * n as f32).ceil() as usize).min(n);
    for i in 0..n {
        let t = (i as f32 + 0.5) / n as f32;
        let seg = if vertical {
            let y1 = inner.bottom() - i as f32 * pitch;
            Rect::from_min_max(Pos2::new(inner.left(), y1 - pitch + 1.0), Pos2::new(inner.right(), y1))
        } else {
            let x0 = inner.left() + i as f32 * pitch;
            Rect::from_min_max(Pos2::new(x0, inner.top()), Pos2::new(x0 + pitch - 1.0, inner.bottom()))
        };
        let c = ladder_colour(&pal, t);
        let on = t <= level || (held > 0 && i + 1 == held);
        ui.painter().rect_filled(seg, 0.5, if on { c } else { c.gamma_multiply(0.13) });
    }
}

/// The width a flowed control needs for `label`.
fn flow_width(ui: &Ui, label: &str, pad: f32) -> f32 {
    let pal = Palette::of(ui.ctx());
    ui.painter().layout_no_wrap(label.to_string(), FontId::proportional(pal.type_label), Color32::WHITE).size().x + pad
}

/// `button` in the flow, as wide as its label needs. Its id comes from its
/// place in the layout, so two with one label never collide.
pub fn flow_button(ui: &mut Ui, label: &str, lit: bool, tint: Color32) -> Response {
    let w = flow_width(ui, label, 18.0);
    let id = ui.next_auto_id();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, FLOW_H), Sense::hover());
    button(ui, rect, label, lit, tint, id)
}

/// `lamp_button` in the flow.
pub fn flow_lamp(ui: &mut Ui, label: &str, on: bool, tint: Color32) -> Response {
    let w = flow_width(ui, label, 28.0);
    let id = ui.next_auto_id();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, FLOW_H), Sense::hover());
    lamp_button(ui, rect, label, on, tint, id)
}

/// `switch` in the flow, each key as wide as its label needs.
pub fn flow_switch(ui: &mut Ui, labels: &[&str], sel: usize, tint: Color32, salt: impl std::hash::Hash) -> Option<usize> {
    let pal = Palette::of(ui.ctx());
    let widths: Vec<f32> = labels
        .iter()
        .map(|l| ui.painter().layout_no_wrap(l.to_string(), FontId::proportional(pal.type_silk), Color32::WHITE).size().x + 16.0)
        .map(|w| w.max(24.0))
        .collect();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(widths.iter().sum(), FLOW_H), Sense::hover());
    switch_keys(ui, rect, labels, &widths, sel, tint, salt)
}

/// `stepper` in the flow, `width` wide.
pub fn flow_stepper(ui: &mut Ui, width: f32, text: &str, salt: impl std::hash::Hash) -> Option<i32> {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, FLOW_H - 2.0), Sense::hover());
    stepper(ui, rect, text, salt)
}

/// A choice among many in a well: the current one shown with a drop mark,
/// a click opens the list. Returns the index picked from the list.
pub fn list_well(ui: &mut Ui, rect: Rect, name_: &str, items: &[&str], selected: usize, tint: Color32, salt: impl std::hash::Hash) -> Option<usize> {
    list_well_response(ui, rect, name_, items, selected, tint, salt).0
}

/// `list_well`, with the well's response, for a hover note on it.
pub fn list_well_response(ui: &mut Ui, rect: Rect, name_: &str, items: &[&str], selected: usize, tint: Color32, salt: impl std::hash::Hash) -> (Option<usize>, Response) {
    let pal = Palette::of(ui.ctx());
    let r = ui.interact(rect, ui.id().with(salt), Sense::click());
    let shown = items.get(selected).copied().unwrap_or("").to_string();
    let label = format!("{name_}: {shown}");
    r.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, true, label.clone()));
    recess(ui, rect, RADIUS);
    if r.hovered() {
        ui.painter().rect_stroke(rect, RADIUS, Stroke::new(1.0_f32, pal.silk_dim.gamma_multiply(0.5)), StrokeKind::Inside);
    }
    let text_rect = rect.shrink2(Vec2::new(6.0, 0.0));
    ui.painter().with_clip_rect(Rect::from_min_max(text_rect.min, Pos2::new(text_rect.right() - 10.0, text_rect.bottom()))).text(
        Pos2::new(text_rect.left(), rect.center().y), Align2::LEFT_CENTER, &shown, FontId::proportional(pal.type_silk + 0.5), tint);
    let tri = Pos2::new(rect.right() - 8.0, rect.center().y);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![tri + Vec2::new(-3.0, -1.5), tri + Vec2::new(3.0, -1.5), tri + Vec2::new(0.0, 2.0)],
        pal.silk_dim, Stroke::NONE));
    let mut out = None;
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(rect.width());
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (i, item) in items.iter().enumerate() {
                let text = egui::RichText::new(*item).color(if i == selected { tint } else { pal.silk });
                if ui.selectable_label(i == selected, text).clicked() {
                    if i != selected {
                        out = Some(i);
                    }
                    ui.close();
                }
            }
        });
    });
    (out, r)
}

/// A dome for a cap or a knob seen from above, lit from above.
pub fn dome(ui: &Ui, centre: Pos2, radius: f32, base: Color32) {
    radial_disc(ui, centre, radius, centre - Vec2::new(0.0, radius * 0.45), lighten(base, 0.22), base.gamma_multiply(0.7));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every control paints, and none hands back a choice unasked.
    #[test]
    fn every_control_paints_and_names_itself() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            {
                let r = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::new(120.0, 22.0));
                assert!(switch(ui, r, &["A", "B"], 0, Color32::RED, "sw").is_none());
                button(ui, r.translate(Vec2::new(0.0, 30.0)), "GO", true, Color32::RED, "b");
                lamp_button(ui, r.translate(Vec2::new(0.0, 60.0)), "ON", true, Color32::RED, "l");
                pad(ui, r.translate(Vec2::new(0.0, 90.0)), "1", Some(Color32::RED), "p");
                assert!(stepper(ui, r.translate(Vec2::new(0.0, 120.0)), "1/8", "s").is_none());
                cap(ui, Rect::from_min_size(Pos2::new(200.0, 10.0), Vec2::new(20.0, 10.0)), Color32::RED, false);
                assert!(fader(ui, Rect::from_min_size(Pos2::new(220.0, 10.0), Vec2::new(10.0, 80.0)), 0.5, Color32::RED, "f").1.is_none());
                ladder(ui, Rect::from_min_size(Pos2::new(240.0, 10.0), Vec2::new(8.0, 80.0)), 0.95, true);
                ladder(ui, Rect::from_min_size(Pos2::new(260.0, 10.0), Vec2::new(80.0, 8.0)), 0.3, false);
                ui.horizontal(|ui| {
                    flow_button(ui, "SAVE", false, Color32::RED);
                    flow_lamp(ui, "SYNC", true, Color32::RED);
                    flow_switch(ui, &["ONE", "TWO"], 1, Color32::RED, "fs");
                    flow_stepper(ui, 60.0, "x", "st");
                });
            }
        });
    }

    /// Look at it: every material and control of the vocabulary on a case,
    /// in the house faces. Ignored because it needs a rendering backend:
    ///   VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
    ///   UPDATE_SNAPSHOTS=1 cargo test -p phonix-ui materials_snapshot -- --ignored
    #[test]
    #[ignore = "needs a rendering backend"]
    fn materials_snapshot() {
        use crate::theme::{backdrop, case, heading, lamp, plate, screw};
        use crate::widgets::{knob_bipolar, knob_fmt, knob_modulated, KnobMod};
        let accent = Color32::from_rgb(0, 200, 255);
        let mut vals = [0.3_f32, 0.72, -0.4, 0.55];
        let mut harness = egui_kittest::Harness::builder().with_size(Vec2::new(760.0, 360.0)).build_ui(move |ui| {
            crate::Palette::DEFAULT.install(ui.ctx());
            crate::Palette::DEFAULT.apply_visuals(ui.ctx());
            let full = ui.max_rect();
            backdrop(ui, full);
            let sheet = case(ui, full.shrink(12.0));
            let a = Rect::from_min_size(sheet.min + Vec2::new(12.0, 12.0), Vec2::new(330.0, 150.0));
            let b = Rect::from_min_size(Pos2::new(a.right() + 12.0, a.top()), Vec2::new(sheet.right() - a.right() - 24.0, 150.0));
            let c = Rect::from_min_max(Pos2::new(a.left(), a.bottom() + 12.0), Pos2::new(b.right(), sheet.bottom() - 12.0));
            for r in [a, b, c] {
                plate(ui, r, 4);
            }
            heading(ui, a.left_top() + Vec2::new(10.0, 12.0), "THE KNOBS", accent, a.right() - 10.0);
            let mut row = ui.new_child(egui::UiBuilder::new().max_rect(a.shrink2(Vec2::new(10.0, 30.0))).layout(egui::Layout::left_to_right(egui::Align::Min)));
            knob_fmt(&mut row, &mut vals[0], "Drive", "30%", 40.0, accent);
            knob_fmt(&mut row, &mut vals[1], "Tone", "72%", 48.0, Color32::from_rgb(255, 140, 60));
            knob_bipolar(&mut row, &mut vals[2], "Pan", 40.0, accent);
            let m = KnobMod { modulated_value: 0.8, source_colors: &[Color32::YELLOW, Color32::LIGHT_GREEN] };
            knob_modulated(&mut row, &mut vals[3], "Cutoff", "2.4k", 40.0, accent, Some(&m));

            heading(ui, b.left_top() + Vec2::new(10.0, 12.0), "SWITCHES AND LAMPS", accent, b.right() - 10.0);
            let x0 = b.left() + 10.0;
            switch(ui, Rect::from_min_size(Pos2::new(x0, b.top() + 30.0), Vec2::new(180.0, 22.0)), &["SINE", "SAW", "PULSE"], 1, accent, "m_sw");
            button(ui, Rect::from_min_size(Pos2::new(x0, b.top() + 62.0), Vec2::new(60.0, 22.0)), "PLAY", true, accent, "m_b1");
            button(ui, Rect::from_min_size(Pos2::new(x0 + 66.0, b.top() + 62.0), Vec2::new(60.0, 22.0)), "STOP", false, accent, "m_b2");
            lamp_button(ui, Rect::from_min_size(Pos2::new(x0 + 132.0, b.top() + 62.0), Vec2::new(70.0, 22.0)), "SYNC", true, accent, "m_l");
            for (i, on) in [true, false].into_iter().enumerate() {
                lamp(ui, Pos2::new(x0 + 220.0 + i as f32 * 20.0, b.top() + 41.0), 4.0, on, Color32::from_rgb(255, 80, 60));
            }
            pad(ui, Rect::from_min_size(Pos2::new(x0, b.top() + 94.0), Vec2::new(44.0, 28.0)), "1", Some(Color32::from_rgb(90, 200, 255)), "m_p1");
            pad(ui, Rect::from_min_size(Pos2::new(x0 + 50.0, b.top() + 94.0), Vec2::new(44.0, 28.0)), "2", None, "m_p2");
            stepper(ui, Rect::from_min_size(Pos2::new(x0 + 104.0, b.top() + 98.0), Vec2::new(70.0, 20.0)), "1/16", "m_st");
            list_well(ui, Rect::from_min_size(Pos2::new(x0 + 184.0, b.top() + 98.0), Vec2::new(110.0, 20.0)), "MODE", &["Hall", "Room"], 0, accent, "m_lw");
            screw(ui, Pos2::new(b.right() - 14.0, b.bottom() - 14.0), 3.2);

            heading(ui, c.left_top() + Vec2::new(10.0, 12.0), "FADERS AND METERS", accent, c.right() - 10.0);
            for (i, lvl) in [0.35_f32, 0.78, 0.97].into_iter().enumerate() {
                ladder(ui, Rect::from_min_size(Pos2::new(c.left() + 14.0 + i as f32 * 16.0, c.top() + 30.0), Vec2::new(9.0, 90.0)), lvl, true);
            }
            ladder(ui, Rect::from_min_size(Pos2::new(c.left() + 80.0, c.top() + 30.0), Vec2::new(160.0, 9.0)), 0.82, false);
            let slot = Rect::from_min_size(Pos2::new(c.left() + 80.0, c.top() + 60.0), Vec2::new(160.0, 8.0));
            recess(ui, slot, 3.0);
            cap(ui, Rect::from_center_size(Pos2::new(slot.left() + 100.0, slot.center().y), Vec2::new(12.0, 20.0)), accent, true);
        });
        harness.run_steps(4);
        harness.snapshot("materials");
    }

    /// The ladder lights the zones in order: safe, then warn, then hot.
    #[test]
    fn the_ladder_runs_safe_warn_hot() {
        let p = Palette::DEFAULT;
        assert_eq!(ladder_colour(&p, 0.1), p.ok);
        assert_eq!(ladder_colour(&p, 0.8), p.warn);
        assert_eq!(ladder_colour(&p, 0.95), p.warm);
    }
}
