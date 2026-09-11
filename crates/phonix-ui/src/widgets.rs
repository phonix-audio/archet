//! Custom egui widgets for Phonix — Design system from doc/GUI.md

use crate::colors::*;
use crate::theme::Palette;
use egui::*;

// ── Knob size constants (doc/GUI.md § Knob) ─────────────────────────
pub const KNOB_SMALL: f32 = 32.0;
pub const KNOB_MEDIUM: f32 = 48.0;
pub const KNOB_LARGE: f32 = 64.0;

// ── Spacing constants (doc/GUI.md § Spacing) ────────────────────────
pub const SECTION_PADDING: f32 = 12.0;
pub const GAP_WIDGETS: f32 = 8.0;
pub const GAP_SECTIONS: f32 = 16.0;
pub const GAP_KNOB_LABEL: f32 = 4.0;
/// UNIFORM knob-group width. Every knob is laid out this wide (or wider only if
/// its diameter exceeds it), INDEPENDENT of the label, so knob rows line up in
/// columns instead of each knob sizing to its own label and shoving neighbours.
/// Fits labels up to ~9 chars at 10pt; longer ones are painted centred and bleed
/// a couple px into the inter-knob gap (no layout displacement). Generalises the
/// `set_min_width(50)` pattern to every editor.
pub const KNOB_GROUP_W: f32 = 52.0;

// ── Shared knob curve + display unit ────────────────────────────────
// One canonical set, used both by hand-written editors and by the
// declarative ui_spec renderer. Replaces the per-plugin private `Unit`
// copies and the informal "curve = which fn you call".

/// How a knob maps its 0..1 rotation onto the real parameter range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Curve {
    #[default]
    Linear,
    Log,
    Bipolar,
    /// Square-root response: more resolution at the BOTTOM of the range than
    /// linear, but far less extreme than log.
    ///
    /// Appended, never reordered. Added for an FM ratio, whose
    /// hand-written knob used exactly this shape (`(v/max).sqrt()`); rendering
    /// it as Log would have given the low end a different feel on a parameter
    /// people tune by ear.
    Sqrt,
}

/// Display unit for a knob's value readout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Unit {
    #[default]
    Pct,
    Hz,
    Ms,
    Sec,
    Semi,
    Cents,
    Int,
    Plain,
    /// Gain in decibels. Appended (never reordered): `Unit` is serialised into
    /// the `.ron` layout specs by variant NAME, but the derive is positional for
    /// any binary format, so new units go at the end.
    Db,
    /// Compression ratio, shown as `6.0:1`.
    Ratio,
    /// A frequency shown as the nearest musical note name (e.g. `A3`), for
    /// pitch controls that offer a Hz/Note display toggle. Appended (never
    /// reordered) per the note above.
    Note,
}

impl Unit {
    /// Render a real value with this unit's suffix (matches the bass_app
    /// `format_value`, the most complete of the pre-existing copies).
    pub fn format(self, v: f32) -> String {
        match self {
            Unit::Pct => format!("{:.0}%", v * 100.0),
            Unit::Hz => {
                if v >= 1000.0 { format!("{:.1}k", v / 1000.0) } else { format!("{:.0} Hz", v) }
            }
            Unit::Ms => {
                if v < 0.01 { format!("{:.1}ms", v * 1000.0) }
                else if v < 1.0 { format!("{:.0}ms", v * 1000.0) }
                else { format!("{:.2}s", v) }
            }
            Unit::Sec => format!("{:.2}s", v),
            Unit::Semi => format!("{:.0} st", v),
            Unit::Cents => format!("{:.0} ct", v),
            Unit::Int => format!("{:.0}", v),
            Unit::Plain => format!("{:.2}", v),
            Unit::Db => format!("{:.1} dB", v),
            Unit::Ratio => format!("{:.1}:1", v),
            Unit::Note => {
                // Nearest equal-tempered note name from a frequency (A4 = 440).
                let midi = (69.0 + 12.0 * (v.max(1e-3) / 440.0).log2()).round() as i32;
                const NAMES: [&str; 12] =
                    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
                let name = NAMES[midi.rem_euclid(12) as usize];
                let octave = midi.div_euclid(12) - 1;
                format!("{name}{octave}")
            }
        }
    }
}

// ── Recursive Panel Layout System ────────────────────────────────────
// ERGO: Gestalt proximity principle — panels group related controls with consistent
// padding (SECTION_PADDING=12) and gaps (GAP_SECTIONS=16 between groups, GAP_WIDGETS=8 within).
// Alternating H/V layout at each depth creates natural visual hierarchy.

/// Content of a panel node: either a leaf closure or child panels.
enum PanelContent<'a> {
    Leaf(Box<dyn FnOnce(&mut Ui) + 'a>),
    Branch(Vec<Panel<'a>>),
}

/// A recursive panel node. Builds a tree of panels that alternate
/// stacking direction at each depth level (horizontal → vertical → ...).
pub struct Panel<'a> {
    header: Option<(&'a str, Color32)>,
    enabled: bool,
    min_col_width: f32,
    /// ERGO: Weight for proportional column sizing. When children have weights,
    /// columns are sized by weight ratio instead of equal widths.
    weight: f32,
    content: PanelContent<'a>,
}

impl<'a> Panel<'a> {
    /// Create a leaf panel — content is laid out horizontally, vertically centered.
    pub fn leaf(content: impl FnOnce(&mut Ui) + 'a) -> Self {
        Self {
            header: None,
            enabled: true,
            min_col_width: 200.0,
            weight: 1.0,
            content: PanelContent::Leaf(Box::new(content)),
        }
    }

    /// Create a branch panel that contains child panels.
    pub fn branch() -> Self {
        Self {
            header: None,
            enabled: true,
            min_col_width: 200.0,
            weight: 1.0,
            content: PanelContent::Branch(Vec::new()),
        }
    }

    /// Set minimum column width before wrapping (default 200.0).
    pub fn min_col_width(mut self, w: f32) -> Self {
        self.min_col_width = w;
        self
    }

    /// ERGO: Set proportional weight for this panel's column width.
    /// e.g. weights [2.0, 3.0, 1.0] → columns sized at 33%, 50%, 17%.
    pub fn weight(mut self, w: f32) -> Self {
        self.weight = w;
        self
    }

    /// Add a section header with accent color.
    pub fn header(mut self, label: &'a str, accent: Color32) -> Self {
        self.header = Some((label, accent));
        self
    }

    /// Set enabled state. Disabled = reduced opacity overlay, content still rendered.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Add a child panel (only meaningful on branch panels).
    pub fn child(mut self, child: Panel<'a>) -> Self {
        if let PanelContent::Branch(ref mut children) = self.content {
            children.push(child);
        }
        self
    }

    /// Render this panel at the given nesting depth.
    /// Even depth = children horizontal, odd depth = children vertical.
    /// Depth 0-1 get border/fill; deeper children are separated by lines.
    pub fn show(self, ui: &mut Ui, depth: u32) {
        if depth >= 1 && self.header.is_some() {
            self.show_framed(ui, depth);
        } else {
            self.show_inner(ui, depth);
        }
    }

    /// Render with border and background (root panels only).
    /// ERGO: Framed panels get a colored left accent stripe matching their header color.
    fn show_framed(self, ui: &mut Ui, depth: u32) {
        let Panel { header, enabled, min_col_width, weight: _, content } = self;
        let accent_color = header.map(|(_, c)| c);
        let pal = Palette::of(ui.ctx());

        let frame_resp = egui::Frame::NONE
            .fill(pal.bg_panel)
            .corner_radius(4.0)
            .inner_margin(12i8)
            .stroke(Stroke::new(1.0_f32, pal.border))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                Self::show_content(ui, header, enabled, min_col_width, content, depth);
            });

        // Left accent stripe on framed panels
        if let Some(color) = accent_color {
            let r = frame_resp.response.rect;
            ui.painter().rect_filled(
                Rect::from_min_size(r.left_top(), Vec2::new(3.0, r.height())),
                egui::CornerRadius { nw: 4, sw: 4, ne: 0, se: 0 },
                color,
            );
        }
    }

    /// Render without border (child panels).
    fn show_inner(self, ui: &mut Ui, depth: u32) {
        let Panel { header, enabled, min_col_width, weight: _, content } = self;
        Self::show_content(ui, header, enabled, min_col_width, content, depth);
    }

    fn show_content(
        ui: &mut Ui,
        header: Option<(&str, Color32)>,
        enabled: bool,
        min_col_width: f32,
        content: PanelContent<'a>,
        depth: u32,
    ) {
        if let Some((label, accent)) = header {
            section_header(ui, label, accent);
        }

        if !enabled {
            let rect_before = ui.cursor();
            let pal = Palette::of(ui.ctx());
            ui.disable();
            Self::render_content(ui, content, min_col_width, depth);
            let full_rect = Rect::from_min_max(
                Pos2::new(rect_before.left(), rect_before.top()),
                Pos2::new(ui.min_rect().right(), ui.min_rect().bottom()),
            );
            ui.painter().rect_filled(
                full_rect,
                4.0,
                Color32::from_rgba_unmultiplied(
                    pal.bg_dark.r(), pal.bg_dark.g(), pal.bg_dark.b(), 140,
                ),
            );
        } else {
            Self::render_content(ui, content, min_col_width, depth);
        }
    }

    fn render_content(ui: &mut Ui, content: PanelContent<'a>, min_col_width: f32, depth: u32) {
        match content {
            PanelContent::Leaf(f) => {
                f(ui);
            }
            PanelContent::Branch(children) => {
                let n = children.len();
                if n == 0 {
                    return;
                }
                if depth % 2 == 0 {
                    let avail_w = ui.available_width();
                    let cols_per_row = ((avail_w / min_col_width).floor() as usize).max(1).min(n);
                    let mut children_opt: Vec<Option<Panel>> =
                        children.into_iter().map(Some).collect();
                    for chunk in children_opt.chunks_mut(cols_per_row) {
                        let chunk_len = chunk.len();
                        ui.columns(chunk_len, |cols| {
                            for (i, child) in chunk.iter_mut().enumerate() {
                                if let Some(child) = child.take() {
                                    child.show(&mut cols[i], depth + 1);
                                }
                            }
                        });
                    }
                } else {
                    // Vertical: children stacked, separated by section gap
                    let mut first = true;
                    for child in children {
                        if !first {
                            if child.header.is_some() {
                                ui.add_space(GAP_SECTIONS);
                            } else {
                                ui.add_space(GAP_WIDGETS);
                                ui.separator();
                                ui.add_space(GAP_WIDGETS);
                            }
                        }
                        first = false;
                        ui.scope(|ui| {
                            child.show(ui, depth + 1);
                        });
                    }
                }
            }
        }
    }
}

/// Draw an ADSR envelope visualization
pub fn adsr_display(
    ui: &mut Ui,
    attack: f32,
    decay: f32,
    sustain: f32,
    release: f32,
    width: f32,
    height: f32,
) {
    let ppp = ui.ctx().pixels_per_point();
    let w = width * ppp / ppp; // keep logical; scaling handled by egui
    let h = height;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let painter = ui.painter_at(rect);

    let pal = Palette::of(ui.ctx());
    painter.rect_filled(rect, 4u8, pal.bg_dark);
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, pal.border), egui::StrokeKind::Outside);

    let pad = 6.0;
    let inner_w = rect.width() - pad * 2.0;
    let inner_h = rect.height() - pad * 2.0;
    let left = rect.left() + pad;
    let top = rect.top() + pad;
    let bottom = rect.bottom() - pad;

    // Proportional to actual ADSR times, with sustain getting a fixed share
    let sustain_frac = 0.15; // sustain hold gets 15% of width
    let adr_w = inner_w * (1.0 - sustain_frac);
    let sum = (attack + decay + release).max(0.001);
    let w_a = adr_w * (attack / sum);
    let w_d = adr_w * (decay / sum);
    let w_s = inner_w * sustain_frac;
    let w_r = adr_w * (release / sum);

    let x_d = left + w_a;
    let x_s = x_d + w_d;
    let x_r = x_s + w_s;

    let sustain_y = top + inner_h * (1.0 - sustain);

    let points = vec![
        Pos2::new(left, bottom),
        Pos2::new(left + w_a, top),
        Pos2::new(x_s, sustain_y),
        Pos2::new(x_r, sustain_y),
        Pos2::new(x_r + w_r, bottom),
    ];

    // Fill: draw triangles from bottom to each line segment
    let fill_color = Color32::from_rgba_unmultiplied(
        ACCENT_ENV.r(), ACCENT_ENV.g(), ACCENT_ENV.b(), 25,
    );
    for pair in points.windows(2) {
        let mut mesh = epaint::Mesh::default();
        mesh.colored_vertex(Pos2::new(pair[0].x, bottom), fill_color);
        mesh.colored_vertex(pair[0], fill_color);
        mesh.colored_vertex(pair[1], fill_color);
        mesh.colored_vertex(Pos2::new(pair[1].x, bottom), fill_color);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(Shape::Mesh(std::sync::Arc::new(mesh)));
    }

    // Stroke: draw the envelope line
    painter.add(Shape::line(
        points,
        Stroke::new(2.0_f32, ACCENT_ENV),
    ));

    // Phase markers (subtle vertical lines)
    let marker_color = Color32::from_rgba_unmultiplied(255, 255, 255, 15);
    for &x in &[x_d, x_s, x_r] {
        painter.line_segment(
            [Pos2::new(x, top), Pos2::new(x, bottom)],
            Stroke::new(1.0_f32, marker_color),
        );
    }

    // ERGO: Phase labels at bottom for quick identification
    let label_color = Color32::from_rgba_unmultiplied(255, 255, 255, 50);
    let label_font = FontId::proportional(8.0);
    let label_y = bottom - 2.0;
    painter.text(Pos2::new(left + w_a / 2.0, label_y), Align2::CENTER_BOTTOM, "A", label_font.clone(), label_color);
    painter.text(Pos2::new(x_d + w_d / 2.0, label_y), Align2::CENTER_BOTTOM, "D", label_font.clone(), label_color);
    painter.text(Pos2::new(x_s + w_s / 2.0, label_y), Align2::CENTER_BOTTOM, "S", label_font.clone(), label_color);
    painter.text(Pos2::new(x_r + w_r / 2.0, label_y), Align2::CENTER_BOTTOM, "R", label_font, label_color);
}

// ── Internal knob drawing (shared between knob and knob_fmt) ────────
// ERGO: Three visual states for knobs — idle (BG_RAISED), hover (accent glow ring),
// active/dragging (full accent arc + pointer movement). Value shown as tooltip on hover
// to keep the idle state clean. Scroll wheel support for fine adjustment.

/// Modulation overlay data passed to `draw_knob_body` when a knob is
/// targeted by the global mod matrix. `modulated_value` is the live
/// (base + Σ routes) value in the same 0..1 space as `value`. Source
/// colors are drawn as small rim dots so users can see which source(s)
/// are moving the knob at a glance.
pub struct KnobMod<'a> {
    pub modulated_value: f32,
    pub source_colors:   &'a [Color32],
}

fn draw_knob_body(
    ui: &mut Ui,
    value: &mut f32,
    label: &str,
    display_text: &str,
    size: f32,
    accent: Color32,
    mod_overlay: Option<&KnobMod<'_>>,
) -> (egui::Response, f32) {
    let s = size;
    let pal = Palette::of(ui.ctx());

    let label_height = 14.0;
    let total_height = s + label_height + GAP_KNOB_LABEL;
    // UNIFORM group width (independent of the label): previously group_w grew to
    // the label, so a long label ("Sub Drone", "Vib Depth", "Destruct") made its
    // knob wider and shoved the following knobs right / broke column alignment
    // across rows. Now every knob of a given size is the same width, so knob rows
    // form aligned columns. Labels up to ~9 chars fit centred; longer ones bleed
    // a couple px into the inter-knob gap (painted, no layout displacement).
    let label_up = label.to_uppercase();
    let group_w = s.max(KNOB_GROUP_W);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(group_w, total_height), Sense::click_and_drag());
    let painter = ui.painter();
    let center = Pos2::new(rect.center().x, rect.top() + s / 2.0);
    let radius = s / 2.0 - 4.0;

    // The label is painted, not laid out, so this is what the accessibility
    // tree sees: a rotary is a slider to a screen reader.
    response.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), *value as f64, label));
    let hovered = response.hovered() || response.dragged();
    let dragging = response.dragged();

    // ERGO: 3-state knob — idle (subtle), hover (glow ring + value), active/drag (bright glow + value)
    if hovered {
        let glow_alpha = if dragging { 140u8 } else { 80 };
        let glow_width = if dragging { 2.5_f32 } else { 1.5_f32 };
        painter.circle_stroke(
            center,
            radius + 5.0,
            Stroke::new(glow_width, Color32::from_rgba_unmultiplied(
                accent.r(), accent.g(), accent.b(), glow_alpha,
            )),
        );
    }

    // Outer ring delimiter
    painter.circle_stroke(center, radius + 3.0, Stroke::new(1.0_f32, pal.border));
    painter.circle_filled(center, radius + 2.0, pal.bg_dark);

    // Background circle
    painter.circle_filled(center, radius, pal.bg_raised);
    painter.circle_stroke(center, radius, Stroke::new(1.0_f32, pal.border));

    // Inner circle (recessed look — darker center)
    painter.circle_filled(center, radius - 3.0, pal.bg_dark);

    // Arc indicator (270 degree sweep)
    let start_angle = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let segments = 48;
    let active_end = start_angle + sweep * *value;

    // Background arc (thicker track)
    for i in 0..segments {
        let t0 = i as f32 / segments as f32;
        let t1 = (i + 1) as f32 / segments as f32;
        let a0 = start_angle + sweep * t0;
        let a1 = start_angle + sweep * t1;
        let r = radius + 1.0;
        let p0 = center + Vec2::new(a0.cos(), a0.sin()) * r;
        let p1 = center + Vec2::new(a1.cos(), a1.sin()) * r;
        painter.line_segment([p0, p1], Stroke::new(3.0_f32, pal.pointer_shadow));
    }

    // Active arc — accent color with glow on hover
    for i in 0..segments {
        let t0 = i as f32 / segments as f32;
        let t1 = (i + 1) as f32 / segments as f32;
        let a0 = start_angle + sweep * t0;
        let a1 = start_angle + sweep * t1;
        if a1 <= active_end {
            let r = radius + 1.0;
            let p0 = center + Vec2::new(a0.cos(), a0.sin()) * r;
            let p1 = center + Vec2::new(a1.cos(), a1.sin()) * r;
            // Thicker active arc + glow when hovered
            let arc_width = if hovered { 3.5_f32 } else { 3.0_f32 };
            painter.line_segment([p0, p1], Stroke::new(arc_width, accent));
            // Soft glow behind active arc on hover
            if hovered {
                let glow_r = radius + 1.0;
                let gp0 = center + Vec2::new(a0.cos(), a0.sin()) * glow_r;
                let gp1 = center + Vec2::new(a1.cos(), a1.sin()) * glow_r;
                painter.line_segment([gp0, gp1], Stroke::new(6.0_f32, Color32::from_rgba_unmultiplied(
                    accent.r(), accent.g(), accent.b(), 30,
                )));
            }
        }
    }

    // ── Modulation overlay (if any) ────────────────────────────────────
    // Draw a thin differential arc between the base value and the live
    // modulated value, plus small colored dots at the outer rim — one
    // per modulation source — so the user can read "this knob is being
    // modulated by LFO 1 and Env 2" at a glance.
    if let Some(m) = mod_overlay {
        let mv = m.modulated_value.clamp(0.0, 1.0);
        let a0 = start_angle + sweep * (*value).clamp(0.0, 1.0);
        let a1 = start_angle + sweep * mv;
        // Differential arc on the outside of the main ring.
        let mod_r = radius + 5.5;
        let seg = 24;
        let (from, to) = if a0 <= a1 { (a0, a1) } else { (a1, a0) };
        for i in 0..seg {
            let t0 = i as f32 / seg as f32;
            let t1 = (i + 1) as f32 / seg as f32;
            let p0 = center + Vec2::new((from + (to - from) * t0).cos(),
                                        (from + (to - from) * t0).sin()) * mod_r;
            let p1 = center + Vec2::new((from + (to - from) * t1).cos(),
                                        (from + (to - from) * t1).sin()) * mod_r;
            let col = Color32::from_rgba_unmultiplied(255, 255, 255, 200);
            painter.line_segment([p0, p1], Stroke::new(2.0_f32, col));
        }
        // Small filled dot at the live-value tip for extra readability.
        let tip = center + Vec2::new(a1.cos(), a1.sin()) * mod_r;
        painter.circle_filled(tip, 2.5, Color32::WHITE);

        // Rim source indicators: evenly-spaced colored dots at the top
        // of the knob so they don't clash with the arc.
        let n = m.source_colors.len().min(6);
        if n > 0 {
            let spread = 0.55; // radians
            let base_a = -std::f32::consts::PI / 2.0;
            let step = if n == 1 { 0.0 } else { spread / (n - 1) as f32 };
            for (i, c) in m.source_colors.iter().take(n).enumerate() {
                let a = base_a - spread / 2.0 + step * i as f32;
                let p = center + Vec2::new(a.cos(), a.sin()) * (radius + 8.0);
                painter.circle_filled(p, 2.0, *c);
            }
        }
    }

    // Pointer line
    let pointer_angle = start_angle + sweep * *value;
    let pointer_start = center + Vec2::new(pointer_angle.cos(), pointer_angle.sin()) * (radius * 0.3);
    let pointer_end = center + Vec2::new(pointer_angle.cos(), pointer_angle.sin()) * (radius - 5.0);
    let pointer_color = if hovered { Color32::WHITE } else { pal.pointer };
    painter.line_segment([pointer_start, pointer_end], Stroke::new(2.0_f32, pointer_color));

    // Pointer dot at tip
    let dot_pos = center + Vec2::new(pointer_angle.cos(), pointer_angle.sin()) * (radius - 5.0);
    painter.circle_filled(dot_pos, if hovered { 2.5 } else { 2.0 }, pointer_color);

    // ERGO: Value shown directly in knob center on hover (no tooltip required)
    if hovered {
        // Show value in center of knob
        let font_size = (s * 0.22).max(8.0).min(12.0);
        painter.text(
            center,
            Align2::CENTER_CENTER,
            display_text,
            FontId::monospace(font_size),
            pal.text_primary,
        );
    }

    // Label: 10px, TEXT_DIM, uppercase (doc/GUI.md § Typography)
    painter.text(
        Pos2::new(center.x, rect.top() + s + GAP_KNOB_LABEL),
        Align2::CENTER_TOP,
        &label_up,
        FontId::proportional(10.0),
        if hovered { pal.text_primary } else { pal.text_dim },
    );

    // Handle drag — supports both vertical and horizontal movement.
    // Uses the LARGER of |dx| and |dy| so horizontal sweeps work too,
    // solving the "cursor hits window top/bottom edge" problem.
    // Shift = fine control (5x precision).
    if response.dragged() {
        let drag = response.drag_delta();
        // Use vertical primarily, but if horizontal motion is larger, use that instead
        // (right = increase, left = decrease — like a horizontal slider)
        let delta = if drag.x.abs() > drag.y.abs() {
            drag.x
        } else {
            -drag.y
        };
        let sensitivity = if ui.input(|i| i.modifiers.shift) { 500.0 } else { 100.0 };
        *value = (*value + delta / sensitivity).clamp(0.0, 1.0);
    }

    // Handle scroll wheel (doc/GUI.md § Knob)
    if hovered {
        let scroll = ui.input(|i| i.smooth_scroll_delta().y);
        if scroll != 0.0 {
            let sensitivity = if ui.input(|i| i.modifiers.shift) { 2000.0 } else { 400.0 };
            let delta = scroll / sensitivity;
            *value = (*value + delta).clamp(0.0, 1.0);
        }
    }

    // ERGO: Double-click resets knob to center (50%)
    if response.double_clicked() {
        *value = 0.5;
    }

    (response, *value)
}

/// Draw a rotary knob with LED ring. Value shown on hover as tooltip.
pub fn knob(ui: &mut Ui, value: &mut f32, label: &str, size: f32, accent: Color32) -> f32 {
    let display = format!("{:.0}%", *value * 100.0);
    draw_knob_body(ui, value, label, &display, size, accent, None).1
}

/// Draw a knob (or any small control) CENTERED in a fixed cell of `width` x
/// `height`. Wrap each knob in a row/grid with the SAME width and their centres
/// line up regardless of label length -- the clean way to get an aligned knob
/// grid without the shared knob auto-sizing to each label. Opt-in per call:
/// pick a `width` >= the widest label in the group; only worth it where the
/// layout has the horizontal room (roomy editors), so tight ones are untouched.
///
/// `height` MUST be the full knob footprint (`size + GAP_KNOB_LABEL + 14`, i.e.
/// `size + 18` for the standard label band). A zero/short height leaves the cell
/// sized only by its content, and inside a `horizontal_wrapped` row that makes
/// egui's cross-axis centring stagger the knobs vertically (each cell centres by
/// its own height). A fixed height makes every cell identical so the row stays
/// flat. Use [`knob_cell_for`] to derive it from a knob `size`.
pub fn knob_cell<R>(ui: &mut Ui, width: f32, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        Vec2::new(width, height),
        Layout::top_down(Align::Center),
        add,
    ).inner
}

/// [`knob_cell`] with the height derived from the knob `size` (the standard
/// `size + GAP_KNOB_LABEL + label band`). Prefer this so callers never mis-size
/// the cell height and reintroduce the wrapped-row stagger.
pub fn knob_cell_for<R>(ui: &mut Ui, width: f32, size: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    knob_cell(ui, width, size + GAP_KNOB_LABEL + 14.0, add)
}

/// Vertically centre a SHORT control (label, checkbox, combo, selector) on the
/// knob-CIRCLE line: reserve a `size`-tall cell (the circle height, NOT the full
/// knob-plus-label footprint) sized to its own content width, and centre the
/// content in it. Use this inside a `horizontal_top` row so short controls line
/// up with the knobs' circles instead of dropping to the row's overall centre
/// (which sits below the circle because of the knob's label band). This is the
/// SAME primitive the declarative renderer uses; hand-drawn Custom closures must
/// use it too so alignment never drifts between the two paths.
pub fn vcell<R>(ui: &mut Ui, size: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        Vec2::new(1.0, size),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_height(size);
            add(ui)
        },
    ).inner
}

/// Knob with custom value display text.
pub fn knob_fmt(ui: &mut Ui, value: &mut f32, label: &str, display_text: &str, size: f32, accent: Color32) -> f32 {
    draw_knob_body(ui, value, label, display_text, size, accent, None).1
}

/// Draw a label in a FIXED-WIDTH slot so its content NEVER reflows the widgets
/// that follow it in a horizontal row. Live-updating status text (a current
/// phoneme, a value readout, an active-voice / step counter) changes width as it
/// updates; placed bare in a `ui.horizontal(..)` it shoves everything after it
/// sideways. This reserves exactly `width` and truncates overflow, so neighbours
/// never move. The shared fix for "text blocks that displace the rest" — use it
/// for any runtime-varying text sitting before other widgets on a row.
pub fn fixed_label(ui: &mut Ui, width: f32, text: RichText) {
    let h = ui.spacing().interact_size.y;
    ui.allocate_ui_with_layout(
        Vec2::new(width, h),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_width(width);
            ui.set_max_width(width);
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

/// Place `add` in a fixed-width cell as tall as a knob's CIRCLE (`knob_size`),
/// vertically centred. A knob reserves a value band UNDER its circle, so a plain
/// label / dropdown / text field beside it (centred on the full row height)
/// drops below the circle. Wrapping the non-knob elements with this, inside a
/// `ui.horizontal_top(..)` row, lines every element up on the knob's centre.
/// The shared fix for "center vertically the elements between themselves".
pub fn knob_aligned<R>(ui: &mut Ui, width: f32, knob_size: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        Vec2::new(width, knob_size),
        Layout::left_to_right(Align::Center),
        |ui| {
            // Reserve the FULL cell (allocate_ui_with_layout otherwise only
            // consumes the content's own size), so a run of these cells with the
            // same width line up in a column: the next element always starts at
            // the same x regardless of this cell's content length.
            ui.set_min_size(Vec2::new(width, knob_size));
            add(ui)
        },
    ).inner
}

/// Log-mapped rotary knob over a `[lo, hi]` range. Returns `true` if the
/// user dragged the knob this frame. Caller passes the real value in
/// real units (Hz, ms, etc.) — the helper handles the normalised
/// mapping and the inverse on read. Use this for filter cutoff,
/// envelope times, anything where a linear sweep would crush the
/// musical region into the bottom 10 % of the rotation.
///
/// Consolidates four near-identical per-plugin log-knob helpers
/// Per-app wrappers should delegate to
/// this primitive so the log math lives in one place.
pub fn knob_log(
    ui: &mut Ui, value: &mut f32, label: &str,
    lo: f32, hi: f32, size: f32, accent: Color32,
) -> bool {
    let lo = lo.max(1e-6);
    let v  = (*value).max(lo);
    let log_range = (hi / lo).ln().max(1e-6);
    let old_norm = ((v / lo).ln() / log_range).clamp(0.0, 1.0);
    let mut norm = old_norm;
    let display = if *value >= 1000.0 { format!("{:.2}k", *value / 1000.0) }
                  else                 { format!("{:.1}",  *value)         };
    draw_knob_body(ui, &mut norm, label, &display, size, accent, None);
    if (norm - old_norm).abs() > 1e-6 {
        *value = (lo * (hi / lo).powf(norm)).clamp(lo, hi);
        true
    } else {
        false
    }
}

/// Bipolar rotary knob over `[-1, +1]` (or any symmetric range). Returns
/// `true` if the user dragged this frame. Owns the encoding /
/// decoding round-trip + the clamp so callsites can't drift.
///
/// Replaces the `(value + 1.0) * 0.5` ↔ `(norm * 2.0 - 1.0).clamp(...)`
/// boilerplate scattered across 20+ pan / detune / mod-amount knobs.
/// Per-app wrappers should delegate here.
pub fn knob_bipolar(
    ui: &mut Ui, value: &mut f32, label: &str, size: f32, accent: Color32,
) -> bool {
    let old = *value;
    let mut norm = ((*value + 1.0) * 0.5).clamp(0.0, 1.0);
    let old_norm = norm;
    let display = if old.abs() < 0.01 { "C".to_string() }
                  else if old > 0.0   { format!("R{:.0}", old * 100.0) }
                  else                { format!("L{:.0}", -old * 100.0) };
    draw_knob_body(ui, &mut norm, label, &display, size, accent, None);
    if (norm - old_norm).abs() > 1e-6 {
        *value = (norm * 2.0 - 1.0).clamp(-1.0, 1.0);
        true
    } else {
        false
    }
}

/// Knob variant that also draws a modulation overlay (differential arc +
/// source rim dots) and returns the `Response` so the caller can attach a
/// context menu ("Modulate with…" / "Remove modulation…").
pub fn knob_modulated(
    ui: &mut Ui,
    value: &mut f32,
    label: &str,
    display_text: &str,
    size: f32,
    accent: Color32,
    mod_overlay: Option<&KnobMod<'_>>,
) -> egui::Response {
    draw_knob_body(ui, value, label, display_text, size, accent, mod_overlay).0
}

/// Small close / remove button drawn entirely with primitives — no font
/// glyph dependency. Two diagonal strokes in a tiny clickable square.
/// Returns the `Response` so callers can attach hover text and check clicks.
pub fn close_button(ui: &mut Ui, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(size, size), Sense::click());
    let hovered = response.hovered();
    let bg = if hovered {
        Color32::from_rgba_unmultiplied(220, 80, 80, 40)
    } else {
        Color32::TRANSPARENT
    };
    let stroke_col = if hovered {
        Color32::from_rgb(240, 120, 120)
    } else {
        Color32::from_gray(150)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 2.0, bg);
    let pad = 3.0;
    let p1 = egui::pos2(rect.min.x + pad, rect.min.y + pad);
    let p2 = egui::pos2(rect.max.x - pad, rect.max.y - pad);
    let p3 = egui::pos2(rect.max.x - pad, rect.min.y + pad);
    let p4 = egui::pos2(rect.min.x + pad, rect.max.y - pad);
    painter.line_segment([p1, p2], Stroke::new(1.5_f32, stroke_col));
    painter.line_segment([p3, p4], Stroke::new(1.5_f32, stroke_col));
    response
}

/// Section header: 11px, ACCENT color, uppercase (doc/GUI.md § Typography)
/// A `DragValue` boxed to a FIXED width so a wider readout (e.g. "10.0k" vs
/// "60 Hz") never shoves the widgets after it in a horizontal row. Use for
/// every in-row numeric editor: knob/label rows then stay aligned within a
/// plugin and read uniformly across plugins. `width` should fit the widest
/// expected value (~56 px ≈ 6 chars is the house default).
pub fn drag_fixed(ui: &mut Ui, dv: DragValue<'_>, width: f32) -> Response {
    ui.add_sized(Vec2::new(width, ui.spacing().interact_size.y), dv)
}

pub fn section_header(ui: &mut Ui, label: &str, accent: Color32) {
    // ERGO: Prominent section header — thick accent bar + larger text + tinted background
    let response = ui.horizontal(|ui| {
        let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(4.0, 18.0), Sense::hover());
        ui.painter().rect_filled(bar_rect, 2u8, accent);
        ui.label(RichText::new(label.to_uppercase()).color(accent).strong().size(12.0));
    });
    // Subtle tinted background behind header row
    let header_rect = response.response.rect;
    ui.painter_at(Rect::from_min_max(header_rect.min, header_rect.max)).rect_filled(
        header_rect.expand2(Vec2::new(4.0, 1.0)),
        2.0,
        Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 12),
    );
    ui.add_space(GAP_KNOB_LABEL + 2.0);
}

/// One option of a segmented selector: a latching pill that fills with the
/// section accent when chosen.
///
/// The declarative renderer used `ui.selectable_label`, which paints the
/// selection with egui's DEFAULT blue regardless of the plugin's accent. Every
/// hand-written selector in this codebase instead fills with the accent (the
/// a vocoder's Male/Female, a bow/pluck switch, the organ footings), so the
/// converted editors were the only ones with a blue square in an amber panel.
pub fn selector_pill(ui: &mut Ui, label: &str, selected: bool, accent: Color32) -> egui::Response {
    let fg = if selected { Color32::WHITE } else { Palette::of(ui.ctx()).text_dim };
    let fill = if selected { accent } else { ui.visuals().widgets.inactive.bg_fill };
    ui.add(
        egui::Button::new(RichText::new(label).color(fg).size(11.0))
            .fill(fill)
            .corner_radius(3.0),
    )
}

/// Section header with accent bar and on/off toggle
pub fn section_header_toggle(ui: &mut Ui, label: &str, accent: Color32, enabled: &mut bool) -> bool {
    let old = *enabled;
    let active_accent = if *enabled { accent } else { Color32::from_rgb(60, 60, 60) };
    let response = ui.horizontal(|ui| {
        let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(4.0, 18.0), Sense::hover());
        ui.painter().rect_filled(bar_rect, 2u8, active_accent);
        let dim = Palette::of(ui.ctx()).text_dim;
        ui.checkbox(enabled, RichText::new(label.to_uppercase()).color(if *enabled { accent } else { dim }).strong().size(12.0));
    });
    if *enabled {
        let header_rect = response.response.rect;
        ui.painter_at(Rect::from_min_max(header_rect.min, header_rect.max)).rect_filled(
            header_rect.expand2(Vec2::new(4.0, 1.0)),
            2.0,
            Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 12),
        );
    }
    ui.add_space(GAP_KNOB_LABEL + 2.0);
    *enabled != old
}

/// Uniform framed section panel: the shared building block every plugin editor
/// should use for a titled block of controls, so the framing, fill, header and
/// left accent stripe look identical across all plugins.
///
/// It fills the width it is given (via set_min_width), so dropping these into
/// the columns of a `ui.columns(n, ..)` yields an aligned, gap-free grid. The
/// body closure runs INLINE (FnOnce), so `&mut self` plugin code composes with
/// it directly, unlike the collect-then-show `Panel` builder. Returns the
/// body's value.
pub fn section_panel<R>(
    ui: &mut Ui,
    title: &str,
    accent: Color32,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    panel_frame(ui, |ui| {
        section_header(ui, title, accent);
        add(ui)
    })
}

/// The shared framed block WITHOUT a header, for a column that stacks several
/// titled sub-sections inside one frame (the sub-sections draw their own
/// section_header). Same house-style fill / border / width fill as
/// [`section_panel`], so a multi-section column frames identically to a
/// single-section one. The section header's own accent bar carries the accent,
/// so the frame deliberately has no extra left stripe (avoids doubling up).
pub fn panel_frame<R>(
    ui: &mut Ui,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let pal = Palette::of(ui.ctx());
    egui::Frame::NONE
        .fill(pal.bg_panel)
        .corner_radius(4.0)
        .inner_margin(10i8)
        .stroke(Stroke::new(1.0_f32, pal.border))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Shared house-style tab strip: draws `labels` as a horizontal row of tab
/// buttons, the one at `selected` highlighted in `accent`. Returns Some(index)
/// when a different tab is clicked. Use for every tabbed editor (main params on
/// the first tab, advanced params on the others) so plugins switch views the
/// same way and dense editors never need to scroll.
pub fn tab_bar(ui: &mut Ui, selected: usize, labels: &[&str], accent: Color32) -> Option<usize> {
    let mut clicked = None;
    let dim = Palette::of(ui.ctx()).text_dim;
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        for (i, label) in labels.iter().enumerate() {
            let is_sel = i == selected;
            // Same weight for all tabs (selection shown by colour + the underline
            // below); bolding only the selected one made it wider and shifted the
            // tabs after it.
            let text = RichText::new(*label).size(11.0)
                .color(if is_sel { accent } else { dim });
            let resp = ui.button(text);
            if resp.clicked() { clicked = Some(i); }
            // Selected-tab underline in the accent, for a proper tab feel.
            if is_sel {
                let r = resp.rect;
                ui.painter().hline(
                    r.left()..=r.right(),
                    r.bottom() + 1.0,
                    Stroke::new(2.0_f32, accent),
                );
            }
            ui.add_space(4.0);
        }
    });
    clicked
}

/// Draw a VU meter with LED segments. Uses ACCENT_GREEN for safe zone.
/// ERGO: Segmented LED VU meter with glow on peak segments for clear level feedback
pub fn vu_meter(ui: &mut Ui, level: f32, width: f32, height: f32, _stereo: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter();

    let pal = Palette::of(ui.ctx());
    painter.rect_filled(rect, 2u8, pal.bg_dark);
    painter.rect_stroke(rect, 2.0, Stroke::new(0.5_f32, pal.border), egui::StrokeKind::Outside);

    let segments = 12;
    let segment_height = (rect.height() - 4.0) / segments as f32;
    let active_segments = (level * segments as f32).round() as i32;

    for i in 0..segments {
        let y = rect.bottom() - 2.0 - (i + 1) as f32 * segment_height;
        let segment_rect = Rect::from_min_size(
            Pos2::new(rect.left() + 2.0, y),
            Vec2::new(rect.width() - 4.0, segment_height - 1.0),
        );

        if i < active_segments {
            let color = if i < 4 { pal.ok } else if i < 8 { pal.warn } else { pal.warm };
            painter.rect_filled(segment_rect, 1u8, color);
            // Glow on the peak segment
            if i == active_segments - 1 {
                painter.rect_filled(
                    segment_rect.expand(1.0),
                    2.0,
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40),
                );
                painter.rect_filled(segment_rect, 1u8, color);
            }
        } else {
            // Dim inactive segments with slight visibility
            painter.rect_filled(segment_rect, 1u8, Color32::from_rgb(28, 28, 32));
        }
    }
}

/// Draw a simple LED indicator
/// ERGO: LED uses two visual states — on (color + glow) and off (dark) for clear status feedback
pub fn led(ui: &mut Ui, on: bool, color: Color32, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    let center = rect.center();
    let radius = size / 2.0;

    if on {
        painter.circle_filled(
            center,
            radius + 2.0,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 40),
        );
    }

    painter.circle_filled(
        center,
        radius,
        if on { color } else { Color32::from_rgb(35, 35, 35) },
    );

    painter.circle_filled(
        Pos2::new(center.x - radius * 0.25, center.y - radius * 0.25),
        radius * 0.25,
        Color32::from_rgba_unmultiplied(255, 255, 255, if on { 80 } else { 20 }),
    );
}

/// Draw a horizontal slider (doc/GUI.md § Slider)
/// ERGO: 3-state slider — idle (dim track), hover (accent glow + larger thumb), drag (bright fill)
pub fn vintage_slider(
    ui: &mut Ui,
    value: &mut f32,
    label: &str,
    min: f32,
    max: f32,
    accent: Color32,
) {
    let pal = Palette::of(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(RichText::new(label.to_uppercase()).color(pal.text_dim).size(10.0));

        let available = (ui.available_width() - 50.0).max(10.0);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(available, 20.0), Sense::click_and_drag());
        let painter = ui.painter();
        let hovered = response.hovered() || response.dragged();
        let dragging = response.dragged();

        // Track: thicker on hover
        let track_h = if hovered { 5.0 } else { 4.0 };
        let track_rect = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), track_h));
        painter.rect_filled(track_rect, 2u8, pal.bg_dark);
        painter.rect_stroke(track_rect, 2u8, Stroke::new(0.5_f32, Color32::from_rgb(20, 20, 22)), egui::StrokeKind::Outside);

        // Filled portion
        let normalized = (*value - min) / (max - min);
        let filled_width = rect.width() * normalized;
        let filled_rect = Rect::from_min_size(track_rect.min, Vec2::new(filled_width, track_h));
        let fill_alpha = if dragging { 180u8 } else if hovered { 140 } else { 102 };
        let fill_color = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), fill_alpha);
        painter.rect_filled(filled_rect, 2u8, fill_color);

        // Glow behind filled portion on hover
        if hovered {
            painter.rect_filled(
                filled_rect.expand(1.5),
                3.0,
                Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 20),
            );
            painter.rect_filled(filled_rect, 2u8, fill_color);
        }

        // Thumb
        let handle_x = rect.left() + filled_width;
        let thumb_center = Pos2::new(handle_x, rect.center().y);
        let thumb_r = if dragging { 7.5 } else if hovered { 7.0 } else { 6.0 };
        painter.circle_filled(thumb_center, thumb_r, if hovered { Color32::WHITE } else { pal.bg_raised });
        painter.circle_stroke(thumb_center, thumb_r, Stroke::new(
            if hovered { 1.5_f32 } else { 1.0_f32 },
            accent,
        ));

        // Value text
        let value_text = format!("{:.2}", *value);
        painter.text(
            Pos2::new(rect.right() + 4.0, rect.center().y),
            Align2::LEFT_CENTER,
            value_text,
            FontId::monospace(12.0),
            if hovered { pal.text_primary } else { pal.text_dim },
        );

        if response.dragged() || response.clicked() {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                let new_normalized = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                *value = min + new_normalized * (max - min);
            }
        }

        // ERGO: Double-click resets to center value
        if response.double_clicked() {
            *value = (min + max) / 2.0;
        }
    });
}

// (Legacy `preset_selector` + `PresetGroup` removed — all callers
//  use `crate::gui::preset_picker::picker_ui` instead.)

// ── Peak meter ───────────────────────────────────────────────────────────
//
// Compact level meter. Pulled out of 5+ plugin headers that all
// re-implemented the same 60x8 rectangle: dim background, filled
// proportionally to peak, red beyond 0.9. Caller passes the size +
// accent so each plugin keeps its identity colour.

/// Draw a horizontal peak meter. `peak` is clamped to 0..1 and used
/// as the fill width fraction. Above 0.9 the fill turns red — the
/// universal convention every plugin already used.
pub fn peak_meter(ui: &mut Ui, peak: f32, size: Vec2, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let bg = Color32::from_rgb(30, 30, 35);
    ui.painter().rect_filled(rect, 2u8, bg);
    let fill_w = peak.clamp(0.0, 1.0) * size.x;
    let fill_rect = Rect::from_min_size(rect.min, Vec2::new(fill_w, size.y));
    let fill_colour = if peak > 0.9 { Color32::RED } else { accent };
    ui.painter().rect_filled(fill_rect, 2u8, fill_colour);
}

/// Vertical peak meter — fills from the bottom up. Used in mixer
/// strips alongside the volume fader so the strip has a real
/// channel-meter look. `peak` is clamped to 0..1; above 0.9 the fill
/// turns red. Includes a faint 0 dB tick at the 0.79 mark (≈ -1 dB
/// from clip) to anchor the eye.
pub fn peak_meter_vertical(ui: &mut Ui, peak: f32, size: Vec2, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2u8, Color32::from_rgb(22, 22, 26));
    let fill_h = peak.clamp(0.0, 1.0) * size.y;
    if fill_h > 0.0 {
        let fill_rect = Rect::from_min_max(
            Pos2::new(rect.min.x, rect.max.y - fill_h),
            rect.max,
        );
        let fill_colour = if peak > 0.9 { Color32::RED } else { accent };
        p.rect_filled(fill_rect, 2u8, fill_colour);
    }
    // 0 dB tick at ~0.79 (≈ -1 dB headroom marker)
    let tick_y = rect.max.y - size.y * 0.79;
    p.line_segment(
        [Pos2::new(rect.min.x, tick_y), Pos2::new(rect.max.x, tick_y)],
        Stroke::new(0.6_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 60)),
    );
}


/// Section header with an icon, no toggle.
///
/// An
/// icon header is a house style, not one plugin's private decoration, and the
/// generic renderer cannot reach a function that lives inside an editor.
pub fn section_header_icon(ui: &mut Ui, icon: &'static [u8], uri: &'static str,
                           title: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        crate::icons::icon_inline(ui, icon, uri, 12.0, color);
        ui.label(RichText::new(title).color(color).size(11.0).strong());
    });
}

/// Section header whose icon and title BOTH act as an on/off switch and both
/// dim when off. Returns true when clicked.
///
/// Dimming the whole header rather than showing a checkbox is what makes a
/// disabled layer readable at a glance across four side-by-side panels.
pub fn panel_header_toggle(ui: &mut Ui, icon: &'static [u8], uri: &'static str,
                           title: &str, color: Color32, enabled: bool) -> bool {
    let mut toggled = false;
    ui.horizontal(|ui| {
        let tint = if enabled { color } else { Palette::of(ui.ctx()).text_dim };
        let img = egui::Image::from_bytes(uri, icon)
            .fit_to_exact_size(egui::Vec2::splat(12.0))
            .tint(tint);
        if ui.add(egui::Button::image(img).frame(false)).clicked() { toggled = true; }
        if ui.add(egui::Label::new(RichText::new(title).color(tint).size(11.0).strong())
            .sense(egui::Sense::click())).clicked() { toggled = true; }
    });
    toggled
}

/// Same, for a section that has no icon.
pub fn panel_header_toggle_plain(ui: &mut Ui, title: &str, color: Color32,
                                 enabled: bool) -> bool {
    // Delegates to `section_header_toggle` so an icon-less toggling header is
    // the SAME header as a hand-written one: accent bar, checkbox, tinted row.
    //
    // It used to be a bare clickable label. That gave the user no affordance at
    // all — nothing said the title could be clicked, and an off section read as
    // dim rather than switched off. One conversion made it obvious: its
    // hand-written sections had checkboxes and the declarative ones did not.
    let mut on = enabled;
    section_header_toggle(ui, title, color, &mut on)
}


// ── Empty-state CTA ───────────────────────────────────────────────────────
//
// Shown in session view / arrangement view / mixer when there are zero
// tracks — gives the user a single obvious "what now" instead of a blank
// panel. The "+ Add your first track" control opens the SAME category-grouped
// Add-Track picker as the sidebar menu, supplied by the caller as `add_menu`
// (it has the session/undo/tx in scope). Previously this was a plain button
// that only set an unread memory flag, so it did nothing.

pub fn empty_session_cta(ui: &mut Ui, view_name: &str, add_menu: impl FnOnce(&mut Ui)) {
    ui.vertical_centered(|ui| {
        ui.add_space(64.0);
        ui.label(RichText::new("No tracks yet").size(18.0).color(Color32::from_gray(180)).strong());
        ui.add_space(6.0);
        ui.label(RichText::new(format!("The {} will fill up once you add an instrument or drum track.", view_name))
            .size(11.0).color(Color32::from_gray(140)));
        ui.add_space(14.0);
        ui.menu_button(
            RichText::new("+ Add your first track").size(13.0).color(Color32::WHITE),
            add_menu,
        );
        ui.add_space(6.0);
        ui.label(RichText::new("Or pick a starter template via File -> New (Ctrl+N).")
            .size(10.0).color(Color32::from_gray(110)).italics());
    });
}


/// Central gate for adopting the engine's published mirror state into a
/// plugin editor's local fields.
///
/// The mirror pattern (engine publishes its full patch ~30 Hz, the GUI adopts
/// it when idle) is only safe if adoption pauses while the user is editing —
/// otherwise the <=33 ms-stale engine echo stomps an in-progress knob drag or
/// text edit (audit v2: rubber-banding knobs, untypeable text fields, across
/// ~10 plugins whose per-knob helpers bypassed the cooldown-arming `send()`).
///
/// Instead of plumbing a cooldown through every knob helper, this gate blocks
/// adoption centrally whenever ANY pointer button is down or a text field has
/// keyboard focus, and for `HOLD` frames after the interaction ends (covering
/// the engine's publish round-trip). Call once per frame from `draw_ui` and
/// only adopt when it returns true. Per-command `sync_cooldown`s remain useful
/// for slow round-trips (preset loads) and compose with this gate.
pub fn mirror_adopt_gate(ctx: &egui::Context, hold_frames: &mut u8) -> bool {
    const HOLD: u8 = 20;
    let interacting = ctx.input(|i| i.pointer.any_down()) || ctx.egui_wants_keyboard_input();
    if interacting {
        *hold_frames = HOLD;
        return false;
    }
    if *hold_frames > 0 {
        *hold_frames -= 1;
        return false;
    }
    true
}

/// A right-click popup that STAYS OPEN while you interact with widgets inside
/// it, including child popups (ComboBox lists, color pickers).
///
/// egui's built-in `Response::context_menu` closes as soon as a pointer press
/// lands outside the menu's own rectangle (egui 0.34 menu.rs `context_interaction`:
/// `!in_old_menu && any_pressed => Close`). A ComboBox or color picker opens its
/// list/picker as a SEPARATE area outside that rectangle, so the first click into
/// it dismisses the whole menu — the menu is unusable for anything but plain
/// buttons. This replacement renders the content in a foreground `Area` and only
/// closes on Escape, on an explicit request from `content` (return `true`, e.g.
/// after an action button), or on a primary click that hits the bare canvas
/// (not this popup and not any child popup — checked via `is_pointer_over_area`).
///
/// One popup is open at a time (keyed by the trigger response id + `salt`).
/// Call every frame with the trigger `Response`; opens on secondary-click.
pub fn sticky_menu(response: &egui::Response, salt: &str, content: impl FnOnce(&mut Ui) -> bool) {
    use egui::Id;
    let ctx = response.ctx.clone();
    let key = Id::new(STICKY_SLOT).with(response.id).with(salt);

    // Open (or move) on secondary click of the trigger.
    let mut just_opened = false;
    if response.secondary_clicked() {
        if let Some(pos) = ctx.pointer_interact_pos() {
            ctx.data_mut(|d| d.insert_temp(Id::new(STICKY_SLOT), (key, pos)));
            just_opened = true;
        }
    }
    sticky_body(&ctx, key, just_opened, content);
}

/// The id under which the one open sticky popup is remembered.
const STICKY_SLOT: &str = "phonix_sticky_menu_open";

/// A menu BUTTON whose popup survives clicking the widgets inside it.
///
/// `ui.menu_button` is only usable for plain buttons: egui closes the menu on
/// any interaction that is not a menu entry, so a text field inside one is
/// dismissed by the very click that would focus it. That was reported twice
/// from the running app, most recently on the Add-Track search field, which made
/// the engine picker unsearchable and hid the VST3 submenu underneath it.
///
/// Shares its close logic with `sticky_menu` through `sticky_body`, so a third
/// report cannot arrive from a third construct. `content` returns true to ask
/// for the menu to close, e.g. after an entry has been chosen.
pub fn sticky_menu_button(ui: &mut Ui, label: &str, content: impl FnOnce(&mut Ui) -> bool) {
    use egui::Id;
    let resp = ui.button(label);
    let ctx = ui.ctx().clone();
    let slot = Id::new(STICKY_SLOT);
    let key = slot.with(resp.id).with(label);

    let mut just_opened = false;
    if resp.clicked() {
        let already_mine = ctx
            .data(|d| d.get_temp::<(Id, egui::Pos2)>(slot))
            .is_some_and(|(k, _)| k == key);
        if already_mine {
            // Clicking the button again closes, as a menu button should.
            ctx.data_mut(|d| d.remove::<(Id, egui::Pos2)>(slot));
            return;
        }
        ctx.data_mut(|d| d.insert_temp(slot, (key, resp.rect.left_bottom())));
        just_opened = true;
    }
    sticky_body(&ctx, key, just_opened, content);
}

/// Draw the open popup, if it is this one, and decide whether it closes.
fn sticky_body(
    ctx: &egui::Context,
    key: egui::Id,
    just_opened: bool,
    content: impl FnOnce(&mut Ui) -> bool,
) {
    use egui::{Area, Order, Id, Pos2, Frame, Key, PointerButton};
    let slot: Id = Id::new(STICKY_SLOT);
    let open: Option<(Id, Pos2)> = ctx.data(|d| d.get_temp(slot));
    let Some((open_key, pos)) = open else { return };
    if open_key != key { return; }

    let area = Area::new(key.with("sticky_area"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            Frame::menu(ui.style()).show(ui, |ui| content(ui)).inner
        });

    let requested_close = area.inner;
    let esc = ctx.input(|i| i.key_pressed(Key::Escape));
    // A primary press on the BARE canvas closes it. A press inside this popup or
    // over any child popup/area leaves it open (that is the whole point).
    let primary_pressed = ctx.input(|i| i.pointer.button_pressed(PointerButton::Primary));
    let inside = ctx.pointer_interact_pos().is_some_and(|p| area.response.rect.contains(p));
    let over_area = ctx.is_pointer_over_egui();
    let clicked_out = primary_pressed && !inside && !over_area;

    if !just_opened && (requested_close || esc || clicked_out) {
        ctx.data_mut(|d| d.remove::<(Id, Pos2)>(slot));
    }
}
