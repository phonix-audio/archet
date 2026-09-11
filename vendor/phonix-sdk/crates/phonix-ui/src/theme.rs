//! The house style as a value, and the painting vocabulary every editor
//! draws with.
//!
//! A `Palette` is installed on the egui context; the shared widgets read
//! it there, so an instrument retunes the whole surface without forking a
//! widget. The vocabulary below takes its colours from the caller or from
//! the palette and never from a constant.

use egui::{Align2, Color32, CornerRadius, FontFamily, FontId, Mesh, Pos2, Rect, Shape, Stroke, StrokeKind, Ui, Vec2};

use crate::colors;

/// The colours the shared widgets read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg_dark: Color32,
    pub bg_panel: Color32,
    pub bg_raised: Color32,
    pub border: Color32,
    pub text_primary: Color32,
    pub text_dim: Color32,
    /// Active states, focus, the default knob pointer.
    pub accent: Color32,
    /// Clipping, a hot state.
    pub warm: Color32,
    /// A safe meter zone, a healthy state.
    pub ok: Color32,
    /// The zone between `ok` and `warm`.
    pub warn: Color32,
    /// A panel of the case: top, bottom, rim.
    pub plate_top: Color32,
    pub plate_bottom: Color32,
    pub plate_edge: Color32,
    /// A well cut into the case, and its rim.
    pub well: Color32,
    pub well_edge: Color32,
    pub lamp_off: Color32,
    /// Silkscreen ink, bright and dim.
    pub silk: Color32,
    pub silk_dim: Color32,
    /// A knob's pointer at rest, and the shadow under it.
    pub pointer: Color32,
    pub pointer_shadow: Color32,
}

impl Palette {
    /// The design-system tokens, as every editor drew them before palettes
    /// were values.
    pub const DEFAULT: Palette = Palette {
        bg_dark: colors::BG_DARK,
        bg_panel: colors::BG_PANEL,
        bg_raised: colors::BG_RAISED,
        border: colors::BORDER,
        text_primary: colors::TEXT_PRIMARY,
        text_dim: colors::TEXT_DIM,
        accent: colors::ACCENT_RGB,
        warm: colors::ACCENT_WARM,
        ok: colors::ACCENT_GREEN,
        warn: colors::LED_YELLOW,
        plate_top: colors::BG_RAISED,
        plate_bottom: colors::BG_PANEL,
        plate_edge: colors::BORDER,
        well: colors::BG_DARK,
        well_edge: Color32::from_rgb(12, 12, 15),
        lamp_off: colors::BG_RAISED,
        silk: colors::TEXT_PRIMARY,
        silk_dim: colors::TEXT_DIM,
        pointer: Color32::from_rgb(200, 200, 210),
        pointer_shadow: Color32::from_rgb(30, 30, 34),
    };

    fn id() -> egui::Id {
        egui::Id::new("phonix_palette")
    }

    /// Makes this the palette every shared widget reads on `ctx`.
    pub fn install(self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.insert_temp(Self::id(), self));
    }

    /// The installed palette, or the default.
    pub fn of(ctx: &egui::Context) -> Palette {
        ctx.data(|d| d.get_temp::<Palette>(Self::id())).unwrap_or(Palette::DEFAULT)
    }

    /// egui's own widgets, tuned to this palette.
    pub fn apply_visuals(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        v.panel_fill = self.bg_dark;
        v.window_fill = self.bg_dark;
        v.extreme_bg_color = self.well;
        v.widgets.noninteractive.bg_fill = self.bg_panel;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, self.text_dim);
        v.widgets.inactive.bg_fill = self.bg_raised;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, self.text_dim);
        v.widgets.hovered.bg_fill = self.bg_raised;
        v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, self.accent.gamma_multiply(0.5));
        v.widgets.active.bg_fill = self.bg_raised;
        v.widgets.active.fg_stroke = Stroke::new(1.0_f32, self.text_primary);
        v.widgets.active.bg_stroke = Stroke::new(1.0_f32, self.accent);
        v.selection.bg_fill = self.accent.gamma_multiply(0.25);
        v.selection.stroke = Stroke::new(1.0_f32, self.accent);
        v.window_stroke = Stroke::new(1.0_f32, self.border);
        ctx.set_visuals(v);
    }
}

impl Default for Palette {
    fn default() -> Self {
        Palette::DEFAULT
    }
}

/// The name of the display face an instrument may register.
pub const DISPLAY: &str = "display";

/// Registers `ttf` as the display face. Call once: `set_fonts` rebuilds
/// every atlas.
pub fn install_display_face(ctx: &egui::Context, ttf: &'static [u8]) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(DISPLAY.to_owned(), std::sync::Arc::new(egui::FontData::from_static(ttf)));
    fonts
        .families
        .entry(FontFamily::Name(DISPLAY.into()))
        .or_default()
        .insert(0, DISPLAY.to_owned());
    ctx.set_fonts(fonts);
}

/// The display face at `size`, or the proportional face while the display
/// face is not bound: `set_fonts` takes effect on the pass after it is
/// called, and asking for an unbound family panics.
pub fn display_font(ctx: &egui::Context, size: f32) -> FontId {
    let bound = ctx.fonts(|f| f.families().iter().any(|fam| matches!(fam, FontFamily::Name(n) if &**n == DISPLAY)));
    if bound {
        FontId::new(size, FontFamily::Name(DISPLAY.into()))
    } else {
        FontId::proportional(size)
    }
}

/// A vertical two-stop gradient, as one mesh quad.
pub fn gradient_v(ui: &Ui, rect: Rect, top: Color32, bottom: Color32) {
    gradient_v_on(ui.painter(), rect, top, bottom)
}

pub fn gradient_v_on(p: &egui::Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(Shape::mesh(mesh));
}

/// A horizontal two-stop gradient.
pub fn gradient_h(ui: &Ui, rect: Rect, left: Color32, right: Color32) {
    gradient_h_on(ui.painter(), rect, left, right)
}

pub fn gradient_h_on(p: &egui::Painter, rect: Rect, left: Color32, right: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    p.add(Shape::mesh(mesh));
}

/// A closed outline filled with a vertical gradient, as a triangle fan from
/// its centroid. Correct for any outline that can see its own centroid,
/// concave ones included.
pub fn fill_polygon(ui: &Ui, pts: &[Pos2], top: Color32, bottom: Color32) {
    if pts.len() < 3 {
        return;
    }
    let n = pts.len() as f32;
    let c = Pos2::new(pts.iter().map(|p| p.x).sum::<f32>() / n, pts.iter().map(|p| p.y).sum::<f32>() / n);
    let y0 = pts.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let y1 = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let shade = |p: Pos2| {
        let t = if y1 > y0 { (p.y - y0) / (y1 - y0) } else { 0.0 };
        lerp_colour(top, bottom, t)
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(c, shade(c));
    for &p in pts {
        mesh.colored_vertex(p, shade(p));
    }
    for i in 1..pts.len() as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    mesh.add_triangle(0, pts.len() as u32, 1);
    ui.painter().add(Shape::mesh(mesh));
}

/// `a` at 0, `b` at 1, premultiplied.
pub fn lerp_colour(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_premultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), mix(a.a(), b.a()))
}

/// A panel of the case: the shadow the lip casts on its top edge and the
/// light that catches its bottom one.
pub fn plate(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius> + Copy) {
    let pal = Palette::of(ui.ctx());
    gradient_v(ui, rect, pal.plate_top, pal.plate_bottom);
    let p = ui.painter();
    p.rect_stroke(rect, radius, Stroke::new(1.0_f32, pal.plate_edge), StrokeKind::Inside);
    p.line_segment(
        [rect.left_top() + Vec2::new(2.0, 1.0), rect.right_top() + Vec2::new(-2.0, 1.0)],
        Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(0, 0, 0, 110)),
    );
    p.line_segment(
        [rect.left_bottom() + Vec2::new(2.0, -1.0), rect.right_bottom() + Vec2::new(-2.0, -1.0)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 20)),
    );
}

/// A well cut into the case: the near wall in shadow, the far wall lit.
pub fn recess(ui: &Ui, rect: Rect, radius: f32) {
    let pal = Palette::of(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, radius, pal.well);
    p.line_segment(
        [rect.left_top() + Vec2::new(radius, 0.5), rect.right_top() + Vec2::new(-radius, 0.5)],
        Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(0, 0, 0, 170)),
    );
    p.line_segment(
        [rect.left_bottom() + Vec2::new(radius, -0.5), rect.right_bottom() + Vec2::new(-radius, -0.5)],
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(255, 255, 255, 24)),
    );
    p.rect_stroke(rect, radius, Stroke::new(1.0_f32, pal.well_edge), StrokeKind::Inside);
}

/// Text screened onto a surface: a pale line under the letters, then the ink.
pub fn printed(ui: &Ui, pos: Pos2, text: &str, font: FontId, ink: Color32, anchor: Align2) {
    ui.painter().text(pos + Vec2::new(0.0, 1.0), anchor, text, font.clone(), Color32::from_rgba_unmultiplied(255, 255, 255, 26));
    ui.painter().text(pos, anchor, text, font, ink);
}

/// Text cut into a surface: a lighter lip below the letters.
pub fn engraved(ui: &Ui, pos: Pos2, text: &str, font: FontId, ink: Color32, anchor: Align2) {
    ui.painter().text(pos + Vec2::new(0.0, 1.0), anchor, text, font.clone(), Color32::from_rgba_unmultiplied(255, 255, 255, 40));
    ui.painter().text(pos, anchor, text, font, ink);
}

/// Text raised off a dark surface: a black line below, the ink above.
pub fn embossed(ui: &Ui, pos: Pos2, text: &str, font: FontId, ink: Color32, anchor: Align2) {
    ui.painter().text(pos + Vec2::new(0.0, 1.0), anchor, text, font.clone(), Color32::from_rgb(8, 8, 8));
    ui.painter().text(pos, anchor, text, font, ink);
}

/// How the letters of `tracked_text` are set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Relief {
    #[default]
    Printed,
    Embossed,
    Flat,
}

/// Letters spaced apart, painted one at a time. Returns the total width.
pub fn tracked_text(ui: &Ui, left: Pos2, text: &str, font: FontId, ink: Color32, tracking: f32, relief: Relief) -> f32 {
    let mut x = left.x;
    for ch in text.chars() {
        let s = ch.to_string();
        let w = ui.painter().layout_no_wrap(s.clone(), font.clone(), ink).size().x;
        let at = Pos2::new(x, left.y);
        match relief {
            Relief::Printed => printed(ui, at, &s, font.clone(), ink, Align2::LEFT_CENTER),
            Relief::Embossed => embossed(ui, at, &s, font.clone(), ink, Align2::LEFT_CENTER),
            Relief::Flat => {
                ui.painter().text(at, Align2::LEFT_CENTER, &s, font.clone(), ink);
            }
        }
        x += w + tracking;
    }
    (x - tracking) - left.x
}

/// A section heading in the silkscreen: tracked capitals over a hairline
/// that runs to at least `rule_to`.
pub fn heading(ui: &Ui, at: Pos2, title: &str, ink: Color32, rule_to: f32) {
    let w = tracked_text(ui, at, title, FontId::proportional(9.0), ink, 1.4, Relief::Printed);
    let y = at.y + 8.0;
    ui.painter().line_segment([Pos2::new(at.x, y), Pos2::new(rule_to.max(at.x + w), y)], Stroke::new(1.0_f32, ink.gamma_multiply(0.35)));
}

/// Small print under a control, one line per line, no wrapping.
pub fn caption(ui: &Ui, at: Pos2, text: &str, ink: Color32) {
    caption_sized(ui, at, text, ink, 8.5)
}

pub fn caption_sized(ui: &Ui, at: Pos2, text: &str, ink: Color32, size: f32) {
    for (i, line) in text.lines().enumerate() {
        ui.painter().text(at + Vec2::new(0.0, i as f32 * (size + 2.0)), Align2::LEFT_TOP, line, FontId::proportional(size), ink);
    }
}

/// A lamp: halo, body, and the glint that sells it as a lens.
pub fn lamp(ui: &Ui, centre: Pos2, radius: f32, on: bool, tint: Color32) {
    let pal = Palette::of(ui.ctx());
    let p = ui.painter();
    if on {
        p.circle_filled(centre, radius * 2.4, tint.gamma_multiply(0.16));
        p.circle_filled(centre, radius * 1.5, tint.gamma_multiply(0.32));
    }
    p.circle_filled(centre, radius, if on { tint } else { pal.lamp_off });
    p.circle_stroke(centre, radius, Stroke::new(1.0_f32, pal.well_edge));
    if on {
        p.circle_filled(centre - Vec2::splat(radius * 0.34), radius * 0.3, Color32::from_rgba_unmultiplied(255, 255, 255, 150));
    }
}

/// A slotted screw head.
pub fn screw(ui: &Ui, centre: Pos2, r: f32) {
    let p = ui.painter();
    p.circle_filled(centre + Vec2::new(0.0, 0.6), r, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
    p.circle_filled(centre, r, Color32::from_rgb(150, 156, 168));
    p.circle_stroke(centre, r, Stroke::new(0.8_f32, Color32::from_rgb(52, 56, 66)));
    p.line_segment([centre - Vec2::new(r * 0.6, 0.0), centre + Vec2::new(r * 0.6, 0.0)], Stroke::new(1.2_f32, Color32::from_rgb(46, 50, 58)));
}

/// A line struck through a rect: a control that is on the panel and inert.
pub fn struck(ui: &Ui, rect: Rect, ink: Color32) {
    ui.painter().line_segment([Pos2::new(rect.left(), rect.center().y), Pos2::new(rect.right(), rect.center().y)], Stroke::new(1.4_f32, ink));
}

/// Diagonal hatching inside a rect, clipped to it.
pub fn hatch(ui: &Ui, rect: Rect, tint: Color32, spacing: f32) {
    if rect.width() < 1.0 || rect.height() < 1.0 {
        return;
    }
    let p = ui.painter().with_clip_rect(rect);
    let span = rect.width() + rect.height();
    let mut x = rect.left() - rect.height();
    while x < rect.left() + span {
        p.line_segment([Pos2::new(x, rect.bottom()), Pos2::new(x + rect.height(), rect.top())], Stroke::new(1.0_f32, tint));
        x += spacing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_without_a_palette_reads_the_default_and_an_installed_one_wins() {
        let ctx = egui::Context::default();
        assert_eq!(Palette::of(&ctx), Palette::DEFAULT);
        let mut warm = Palette::DEFAULT;
        warm.accent = Color32::from_rgb(197, 160, 88);
        warm.install(&ctx);
        assert_eq!(Palette::of(&ctx).accent, Color32::from_rgb(197, 160, 88));
        assert_eq!(Palette::of(&ctx).bg_dark, Palette::DEFAULT.bg_dark);
    }

    #[test]
    fn the_default_palette_is_dark_with_light_ink() {
        let lum = |c: Color32| 0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32;
        let p = Palette::DEFAULT;
        assert!(lum(p.bg_dark) < lum(p.bg_panel) && lum(p.bg_panel) < lum(p.bg_raised));
        assert!(lum(p.text_dim) < lum(p.text_primary));
        assert!(lum(p.well) <= lum(p.bg_dark));
    }

    #[test]
    fn the_vocabulary_paints_without_panicking() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 40.0));
                plate(ui, r, 3.0);
                recess(ui, r.shrink(4.0), 2.0);
                heading(ui, r.left_top(), "SECTION", Color32::WHITE, r.right());
                caption(ui, r.left_bottom(), "one\ntwo", Color32::GRAY);
                lamp(ui, r.center(), 4.0, true, Color32::RED);
                screw(ui, r.right_top(), 3.0);
                hatch(ui, r, Color32::GRAY, 4.0);
                struck(ui, r, Color32::RED);
                fill_polygon(ui, &[r.left_top(), r.right_top(), r.center_bottom()], Color32::RED, Color32::BLUE);
                let w = tracked_text(ui, r.left_center(), "AB", FontId::proportional(10.0), Color32::WHITE, 2.0, Relief::Embossed);
                assert!(w > 0.0);
            });
        });
    }
}
