//! The house style as a value, and the painting vocabulary every editor
//! draws with.
//!
//! A `Palette` is installed on the egui context; the shared widgets read
//! it there, so an instrument retunes the whole surface without forking a
//! widget. The vocabulary below takes its colours from the caller or from
//! the palette and never from a constant.

use egui::epaint::{CornerRadiusF32, PathShape};
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
    /// The one light, from above: the edge it catches on a raised part,
    /// the edge it leaves dark, and the shadow a raised part casts below.
    pub light_edge: Color32,
    pub shade_edge: Color32,
    pub shadow: Color32,
    /// The room behind the instrument, top and bottom.
    pub backdrop_top: Color32,
    pub backdrop_bottom: Color32,
    /// The sheet the plates sit on, top and bottom.
    pub case_top: Color32,
    pub case_bottom: Color32,
    /// The machined cheeks at the ends of the case, and their grain.
    pub cheek_top: Color32,
    pub cheek_bottom: Color32,
    pub cheek_grain: Color32,
    /// A screw head, lit and in shade.
    pub screw_lit: Color32,
    pub screw_shade: Color32,
    /// The preset's nameplate in the header: its face, its rolled edge and
    /// the ink engraved in it. A transparent face leaves it a well.
    pub nameplate_face: Color32,
    pub nameplate_edge: Color32,
    pub nameplate_ink: Color32,
    /// The type scale in points: a section heading, silkscreen beside a
    /// control, a caption under it, a control's label, a value readout.
    pub type_heading: f32,
    pub type_silk: f32,
    pub type_caption: f32,
    pub type_label: f32,
    pub type_value: f32,
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
        light_edge: Color32::from_rgba_premultiplied(30, 30, 30, 30),
        shade_edge: Color32::from_rgba_premultiplied(0, 0, 0, 120),
        shadow: Color32::from_rgba_premultiplied(0, 0, 0, 120),
        backdrop_top: Color32::from_rgb(30, 30, 35),
        backdrop_bottom: Color32::from_rgb(12, 12, 15),
        case_top: Color32::from_rgb(40, 40, 46),
        case_bottom: Color32::from_rgb(27, 27, 32),
        cheek_top: Color32::from_rgb(70, 71, 78),
        cheek_bottom: Color32::from_rgb(38, 39, 44),
        cheek_grain: Color32::from_rgba_premultiplied(0, 0, 0, 40),
        screw_lit: Color32::from_rgb(178, 182, 192),
        screw_shade: Color32::from_rgb(92, 96, 106),
        nameplate_face: Color32::TRANSPARENT,
        nameplate_edge: Color32::TRANSPARENT,
        nameplate_ink: Color32::TRANSPARENT,
        type_heading: 9.5,
        type_silk: 9.0,
        type_caption: 8.5,
        type_label: 9.5,
        type_value: 10.0,
    };

    fn id() -> egui::Id {
        egui::Id::new("phonix_palette")
    }

    /// Makes this the palette every shared widget reads on `ctx`, and the
    /// house faces the ones its text is set in.
    pub fn install(self, ctx: &egui::Context) {
        ctx.data_mut(|d| d.insert_temp(Self::id(), self));
        crate::fonts::install(ctx);
    }

    /// The installed palette, or the default.
    pub fn of(ctx: &egui::Context) -> Palette {
        ctx.data(|d| d.get_temp::<Palette>(Self::id())).unwrap_or(Palette::DEFAULT)
    }

    /// egui's own widgets, tuned to this palette: its popups, tooltips and
    /// menus are small plates lit from above, its scrollbars sit in the
    /// well's colours.
    pub fn apply_visuals(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        v.window_corner_radius = CornerRadius::same(5);
        v.menu_corner_radius = CornerRadius::same(4);
        v.window_shadow = egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: self.shadow };
        v.popup_shadow = egui::Shadow { offset: [0, 3], blur: 10, spread: 0, color: self.shadow };
        v.faint_bg_color = self.bg_panel;
        v.code_bg_color = self.well;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, self.well_edge);
        v.widgets.inactive.weak_bg_fill = self.bg_raised;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, self.well_edge);
        v.widgets.hovered.weak_bg_fill = self.bg_raised;
        v.widgets.active.weak_bg_fill = self.bg_raised;
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = CornerRadius::same(3);
        }
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
        ctx.global_style_mut(|s| {
            s.spacing.scroll = egui::style::ScrollStyle::thin();
            s.spacing.menu_margin = egui::Margin::same(6);
            s.spacing.item_spacing = Vec2::new(6.0, 4.0);
        });
    }
}

impl Default for Palette {
    fn default() -> Self {
        Palette::DEFAULT
    }
}

/// The name of the display face an instrument may register.
pub const DISPLAY: &str = "display";

/// Registers `ttf` as the display face, beside the house faces. Cheap to
/// call every frame: the fonts are set only when the face changes.
pub fn install_display_face(ctx: &egui::Context, ttf: &'static [u8]) {
    crate::fonts::set_display(ctx, ttf);
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

/// A rounded rect filled with a vertical gradient, its edge feathered over
/// one pixel as egui feathers its own shapes. The fill and a stroke drawn
/// with the same radius share their corners.
pub fn gradient_rounded(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius>, top: Color32, bottom: Color32) {
    gradient_rounded_on(ui.painter(), rect, radius, top, bottom)
}

pub fn gradient_rounded_on(p: &egui::Painter, rect: Rect, radius: impl Into<CornerRadius>, top: Color32, bottom: Color32) {
    let mut pts = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(&mut pts, rect, CornerRadiusF32::from(radius.into()));
    let feather = 1.0 / p.ctx().pixels_per_point();
    p.add(Shape::mesh(feathered_fan(&pts, rect.center(), feather, |q| {
        let t = if rect.height() > 0.0 { (q.y - rect.top()) / rect.height() } else { 0.0 };
        lerp_colour(top, bottom, t)
    })));
}

/// A disc shaded from `inner` at `focus` to `outer` at its rim: the dome of
/// a lens, a cap or a screw head lit from where `focus` sits.
pub fn radial_disc(ui: &Ui, centre: Pos2, radius: f32, focus: Pos2, inner: Color32, outer: Color32) {
    if radius <= 0.0 {
        return;
    }
    let n = ((radius * 2.0) as usize).clamp(16, 96);
    let pts: Vec<Pos2> = (0..n)
        .map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            centre + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    let feather = 1.0 / ui.ctx().pixels_per_point();
    let reach = radius + (focus - centre).length();
    ui.painter().add(Shape::mesh(feathered_fan(&pts, focus, feather, |q| {
        lerp_colour(inner, outer, ((q - focus).length() / reach.max(0.001)).powf(0.8))
    })));
}

/// A convex outline as a fan from `hub`, each vertex coloured by `shade`,
/// with a ring of transparent vertices `feather` outside it so the edge is
/// antialiased.
fn feathered_fan(pts: &[Pos2], hub: Pos2, feather: f32, shade: impl Fn(Pos2) -> Color32) -> Mesh {
    let mut mesh = Mesh::default();
    let n = pts.len();
    if n < 3 {
        return mesh;
    }
    let centroid = Pos2::new(pts.iter().map(|p| p.x).sum::<f32>() / n as f32, pts.iter().map(|p| p.y).sum::<f32>() / n as f32);
    mesh.colored_vertex(hub, shade(hub));
    for i in 0..n {
        let (prev, here, next) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
        let normal = |a: Pos2, b: Pos2| {
            let d = b - a;
            let nrm = Vec2::new(d.y, -d.x).normalized();
            if nrm.dot((a + d * 0.5) - centroid) < 0.0 { -nrm } else { nrm }
        };
        let mut out = normal(prev, here) + normal(here, next);
        out = if out.length() > 1e-4 { out.normalized() } else { (here - centroid).normalized() };
        let c = shade(here);
        mesh.colored_vertex(here - out * feather * 0.5, c);
        mesh.colored_vertex(here + out * feather * 0.5, Color32::TRANSPARENT);
    }
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        let (ii, io, ji, jo) = (1 + 2 * i, 2 + 2 * i, 1 + 2 * j, 2 + 2 * j);
        mesh.add_triangle(0, ii, ji);
        mesh.add_triangle(ii, io, ji);
        mesh.add_triangle(io, jo, ji);
    }
    mesh
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

/// `c` lifted towards white by `t`, its alpha kept.
pub fn lighten(c: Color32, t: f32) -> Color32 {
    let w = Color32::from_rgba_premultiplied(c.a(), c.a(), c.a(), c.a());
    lerp_colour(c, w, t)
}

/// The shadow a raised part of `rect` casts on what it sits on, from the
/// light above it.
pub fn cast_shadow(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius>, lift: f32) {
    let pal = Palette::of(ui.ctx());
    let shadow = egui::Shadow {
        offset: [0, lift.round().clamp(0.0, 12.0) as i8],
        blur: (lift * 3.0).round().clamp(0.0, 40.0) as u8,
        spread: 0,
        color: pal.shadow,
    };
    ui.painter().add(shadow.as_shape(rect, radius));
}

/// The light caught by the top edge of a raised part, and the shade along
/// its bottom one, inside `rect`.
pub fn lit_edges(ui: &Ui, rect: Rect, radius: f32) {
    let pal = Palette::of(ui.ctx());
    let p = ui.painter();
    let inset = radius.max(1.5);
    p.line_segment(
        [Pos2::new(rect.left() + inset, rect.top() + 1.0), Pos2::new(rect.right() - inset, rect.top() + 1.0)],
        Stroke::new(1.0_f32, pal.light_edge),
    );
    p.line_segment(
        [Pos2::new(rect.left() + inset, rect.bottom() - 1.0), Pos2::new(rect.right() - inset, rect.bottom() - 1.0)],
        Stroke::new(1.0_f32, pal.shade_edge),
    );
}

/// A panel of the case, raised off it: the shadow it casts, a gradient
/// lit from above, the light its top edge catches and its rim.
pub fn plate(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius> + Copy) {
    ui.painter().extend(plate_shapes(ui.ctx(), rect, radius.into()));
}

/// The shapes `plate` paints, for a caller that places them under content
/// laid out first.
pub fn plate_shapes(ctx: &egui::Context, rect: Rect, cr: CornerRadius) -> Vec<Shape> {
    let pal = Palette::of(ctx);
    let feather = 1.0 / ctx.pixels_per_point();
    let mut pts = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(&mut pts, rect, CornerRadiusF32::from(cr));
    let fill = feathered_fan(&pts, rect.center(), feather, |q| {
        let t = if rect.height() > 0.0 { (q.y - rect.top()) / rect.height() } else { 0.0 };
        lerp_colour(pal.plate_top, pal.plate_bottom, t)
    });
    let inset = cr.average().max(1.5);
    vec![
        Shape::Rect(egui::Shadow { offset: [0, 2], blur: 6, spread: 0, color: pal.shadow }.as_shape(rect, cr)),
        Shape::mesh(fill),
        Shape::line_segment(
            [Pos2::new(rect.left() + inset, rect.top() + 1.0), Pos2::new(rect.right() - inset, rect.top() + 1.0)],
            Stroke::new(1.0_f32, pal.light_edge),
        ),
        Shape::line_segment(
            [Pos2::new(rect.left() + inset, rect.bottom() - 1.0), Pos2::new(rect.right() - inset, rect.bottom() - 1.0)],
            Stroke::new(1.0_f32, pal.shade_edge),
        ),
        Shape::Rect(egui::epaint::RectShape::stroke(rect, cr, Stroke::new(1.0_f32, pal.plate_edge), StrokeKind::Inside)),
    ]
}

/// Content laid out on a plate: the plate is painted under it once its
/// size is known, `margin` inside its rim.
pub fn plated<R>(ui: &mut Ui, radius: u8, margin: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    let under = ui.painter().add(Shape::Noop);
    let inner = egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        add(ui)
    });
    let shapes = plate_shapes(ui.ctx(), inner.response.rect, CornerRadius::same(radius));
    ui.painter().set(under, Shape::Vec(shapes));
    inner.inner
}

/// A well cut into the case: its near wall throws a soft shadow down into
/// it, its far wall and the lip below it catch the light.
pub fn recess(ui: &Ui, rect: Rect, radius: f32) {
    let pal = Palette::of(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, radius, pal.well);
    let inset = radius.max(1.0);
    for (k, alpha) in [(0.5_f32, 150u8), (1.5, 80), (2.5, 36), (3.5, 14)] {
        if rect.height() < k * 2.0 + 2.0 {
            break;
        }
        let y = rect.top() + k;
        p.line_segment(
            [Pos2::new(rect.left() + inset, y), Pos2::new(rect.right() - inset, y)],
            Stroke::new(1.0_f32, Color32::from_rgba_premultiplied(0, 0, 0, alpha)),
        );
    }
    p.line_segment(
        [Pos2::new(rect.left() + inset, rect.bottom() - 0.5), Pos2::new(rect.right() - inset, rect.bottom() - 0.5)],
        Stroke::new(1.0_f32, pal.light_edge),
    );
    p.rect_stroke(rect, radius, Stroke::new(1.0_f32, pal.well_edge), StrokeKind::Inside);
    p.line_segment(
        [Pos2::new(rect.left() + inset, rect.bottom() + 0.5), Pos2::new(rect.right() - inset, rect.bottom() + 0.5)],
        Stroke::new(1.0_f32, pal.light_edge.gamma_multiply(0.7)),
    );
}

/// A plate of rolled metal: dark at its edges, bright across its middle,
/// with its rim. What the palette's nameplate is struck from.
pub fn nameplate(ui: &Ui, rect: Rect, radius: f32) {
    let pal = Palette::of(ui.ctx());
    cast_shadow(ui, rect, radius as u8, 1.5);
    let upper = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.center().y));
    let lower = Rect::from_min_max(Pos2::new(rect.left(), rect.center().y), rect.max);
    let top = CornerRadius { nw: radius as u8, ne: radius as u8, sw: 0, se: 0 };
    let bottom = CornerRadius { nw: 0, ne: 0, sw: radius as u8, se: radius as u8 };
    gradient_rounded(ui, upper, top, pal.nameplate_edge, pal.nameplate_face);
    gradient_rounded(ui, lower, bottom, pal.nameplate_face, pal.nameplate_edge);
    ui.painter().rect_stroke(rect, radius, Stroke::new(1.0_f32, pal.shade_edge), StrokeKind::Inside);
}

/// The room behind the instrument: a gradient, darker at its sides.
pub fn backdrop(ui: &Ui, rect: Rect) {
    let pal = Palette::of(ui.ctx());
    gradient_v(ui, rect, pal.backdrop_top, pal.backdrop_bottom);
    let side = (rect.width() * 0.08).min(80.0);
    let dark = Color32::from_rgba_premultiplied(0, 0, 0, 70);
    gradient_h(ui, Rect::from_min_size(rect.min, Vec2::new(side, rect.height())), dark, Color32::TRANSPARENT);
    gradient_h(ui, Rect::from_min_max(Pos2::new(rect.right() - side, rect.top()), rect.max), Color32::TRANSPARENT, dark);
}

/// A machined, brushed surface: `top` to `bottom`, with the fine grain the
/// brush leaves across it. The grain is seeded by the rect, so a surface
/// keeps its grain from frame to frame.
pub fn brushed(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius> + Copy, top: Color32, bottom: Color32, grain: Color32) {
    gradient_rounded(ui, rect, radius, top, bottom);
    let p = ui.painter().with_clip_rect(rect.shrink(1.0));
    let mut y = rect.top() + 1.0;
    let mut k = (rect.left() * 7.0 + rect.top() * 13.0) as u32;
    while y < rect.bottom() - 1.0 {
        k = k.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let a = 0.35 + (k >> 24) as f32 / 255.0 * 0.65;
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0_f32, grain.gamma_multiply(a)));
        y += 2.0;
    }
}

/// The width of a cheek of the case.
pub const CHEEK_W: f32 = 16.0;

/// The case the plates are set into: its shadow, the sheet, a machined
/// cheek at each end with a screw near each corner. Returns the sheet
/// between the cheeks.
pub fn case(ui: &Ui, rect: Rect) -> Rect {
    let pal = Palette::of(ui.ctx());
    let r = 6u8;
    cast_shadow(ui, rect, r, 4.0);
    gradient_rounded(ui, rect, r, pal.case_top, pal.case_bottom);
    let left = Rect::from_min_size(rect.min, Vec2::new(CHEEK_W, rect.height()));
    let right = Rect::from_min_size(Pos2::new(rect.right() - CHEEK_W, rect.top()), Vec2::new(CHEEK_W, rect.height()));
    brushed(ui, left, CornerRadius { nw: r, sw: r, ne: 0, se: 0 }, pal.cheek_top, pal.cheek_bottom, pal.cheek_grain);
    brushed(ui, right, CornerRadius { nw: 0, sw: 0, ne: r, se: r }, pal.cheek_top, pal.cheek_bottom, pal.cheek_grain);
    let p = ui.painter();
    for x in [left.right(), right.left()] {
        p.line_segment([Pos2::new(x, rect.top() + 1.0), Pos2::new(x, rect.bottom() - 1.0)], Stroke::new(1.0_f32, pal.shade_edge));
    }
    lit_edges(ui, rect, r as f32);
    p.rect_stroke(rect, r, Stroke::new(1.0_f32, pal.shade_edge), StrokeKind::Inside);
    for y in [rect.top() + 14.0, rect.bottom() - 14.0] {
        screw(ui, Pos2::new(left.center().x, y), 3.2);
        screw(ui, Pos2::new(right.center().x, y), 3.2);
    }
    Rect::from_min_max(Pos2::new(left.right(), rect.top()), Pos2::new(right.left(), rect.bottom()))
}

/// Text screened onto a surface: a pale line under the letters, then the ink.
pub fn printed(ui: &Ui, pos: Pos2, text: &str, font: FontId, ink: Color32, anchor: Align2) {
    ui.painter().text(pos + Vec2::new(0.0, 1.0), anchor, text, font.clone(), Color32::from_rgba_unmultiplied(255, 255, 255, 18));
    ui.painter().text(pos, anchor, text, font, ink);
}

/// Text cut into a surface: a lighter lip below the letters.
pub fn engraved(ui: &Ui, pos: Pos2, text: &str, font: FontId, ink: Color32, anchor: Align2) {
    ui.painter().text(pos + Vec2::new(0.0, 1.0), anchor, text, font.clone(), Color32::from_rgba_unmultiplied(255, 255, 255, 34));
    ui.painter().text(pos + Vec2::new(0.0, -0.5), anchor, text, font.clone(), Color32::from_rgba_unmultiplied(0, 0, 0, 90));
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

/// The width `text` takes set with `tracking` between its letters.
pub fn tracked_width(ui: &Ui, text: &str, font: FontId, tracking: f32) -> f32 {
    let mut w = 0.0;
    let mut n = 0;
    for ch in text.chars() {
        w += ui.painter().layout_no_wrap(ch.to_string(), font.clone(), Color32::WHITE).size().x;
        n += 1;
    }
    w + tracking * (n.max(1) - 1) as f32
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
    let pal = Palette::of(ui.ctx());
    let w = tracked_text(ui, at, title, FontId::proportional(pal.type_heading), ink, 1.4, Relief::Printed);
    let y = at.y + 8.5;
    let p = ui.painter();
    p.line_segment([Pos2::new(at.x, y), Pos2::new(rule_to.max(at.x + w), y)], Stroke::new(1.0_f32, ink.gamma_multiply(0.35)));
    p.line_segment([Pos2::new(at.x, y + 1.0), Pos2::new(rule_to.max(at.x + w), y + 1.0)], Stroke::new(1.0_f32, pal.light_edge.gamma_multiply(0.6)));
}

/// Small print under a control, one line per line, no wrapping.
pub fn caption(ui: &Ui, at: Pos2, text: &str, ink: Color32) {
    let size = Palette::of(ui.ctx()).type_caption;
    caption_sized(ui, at, text, ink, size)
}

pub fn caption_sized(ui: &Ui, at: Pos2, text: &str, ink: Color32, size: f32) {
    for (i, line) in text.lines().enumerate() {
        ui.painter().text(at + Vec2::new(0.0, i as f32 * (size + 2.5)), Align2::LEFT_TOP, line, FontId::proportional(size), ink);
    }
}

/// A lamp: a lens set in a bezel. Lit, it glows through a falloff of light
/// around it; dark, it is still glass, with its dome and glint.
pub fn lamp(ui: &Ui, centre: Pos2, radius: f32, on: bool, tint: Color32) {
    let pal = Palette::of(ui.ctx());
    let p = ui.painter();
    if on {
        for k in (1..=6).rev() {
            let f = k as f32 / 6.0;
            p.circle_filled(centre, radius * (1.0 + 1.6 * f), tint.gamma_multiply(0.07 * (1.0 - f * 0.7)));
        }
    }
    p.circle_filled(centre, radius + 1.0, pal.well_edge);
    let focus = centre - Vec2::new(radius * 0.3, radius * 0.35);
    if on {
        radial_disc(ui, centre, radius, focus, lighten(tint, 0.55), tint.gamma_multiply(0.75));
    } else {
        radial_disc(ui, centre, radius, focus, lighten(pal.lamp_off, 0.18), lerp_colour(pal.lamp_off, pal.well, 0.6));
    }
    let glint = if on { 170 } else { 55 };
    p.circle_filled(centre - Vec2::new(radius * 0.32, radius * 0.38), radius * 0.28, Color32::from_rgba_unmultiplied(255, 255, 255, glint));
}

/// A slotted screw head, turned: domed and lit from above, its slot at an
/// angle of its own, so the screws of a case do not all line up.
pub fn screw(ui: &Ui, centre: Pos2, r: f32) {
    let pal = Palette::of(ui.ctx());
    let p = ui.painter();
    p.circle_filled(centre + Vec2::new(0.0, 0.8), r + 0.6, pal.shadow);
    radial_disc(ui, centre, r, centre - Vec2::new(r * 0.3, r * 0.45), pal.screw_lit, pal.screw_shade);
    p.circle_stroke(centre, r, Stroke::new(0.8_f32, pal.well_edge));
    let seed = ((centre.x * 12.9898 + centre.y * 78.233).sin() * 43_758.547).fract().abs();
    let a = seed * std::f32::consts::PI;
    let d = Vec2::new(a.cos(), a.sin()) * r * 0.72;
    p.line_segment([centre - d, centre + d], Stroke::new(1.3_f32, pal.well_edge));
    p.line_segment([centre - d + Vec2::new(0.0, 0.7), centre + d + Vec2::new(0.0, 0.7)], Stroke::new(0.6_f32, pal.light_edge));
}

/// An arc of `radius` round `centre` from angle `a0` to `a1`, as one path.
pub fn arc(ui: &Ui, centre: Pos2, radius: f32, a0: f32, a1: f32, stroke: Stroke) {
    if (a1 - a0).abs() < 1e-4 {
        return;
    }
    let n = (((a1 - a0).abs() * radius / 2.0) as usize).clamp(4, 128);
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            centre + Vec2::new(a.cos(), a.sin()) * radius
        })
        .collect();
    ui.painter().add(PathShape::line(pts, stroke));
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

    /// The rounded gradient's solid vertices stay inside the rounded rect,
    /// and only its feather ring reaches half a pixel outside it.
    #[test]
    fn the_rounded_gradient_keeps_to_its_corners() {
        let rect = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(80.0, 30.0));
        let mut pts = Vec::new();
        egui::epaint::tessellator::path::rounded_rectangle(&mut pts, rect, CornerRadiusF32::same(6.0));
        let mesh = feathered_fan(&pts, rect.center(), 1.0, |_| Color32::WHITE);
        let inside = |q: Pos2, slack: f32| {
            let r = 6.0;
            let cx = q.x.clamp(rect.left() + r, rect.right() - r);
            let cy = q.y.clamp(rect.top() + r, rect.bottom() - r);
            (q - Pos2::new(cx, cy)).length() <= r + slack
        };
        for v in &mesh.vertices {
            if v.color == Color32::TRANSPARENT {
                assert!(inside(v.pos, 0.51), "feather vertex {:?} strayed", v.pos);
            } else {
                assert!(inside(v.pos, 0.01), "solid vertex {:?} left the rounded rect", v.pos);
            }
        }
        assert!(!inside(Pos2::new(rect.left() + 0.5, rect.top() + 0.5), 0.0), "a corner is cut");
    }

    /// Content on a plate takes the width it is given, margins included,
    /// and no more.
    #[test]
    fn a_plate_keeps_to_the_width_it_is_given() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let r = ui.allocate_ui(Vec2::new(300.0, 200.0), |ui| {
                ui.set_max_width(300.0);
                plated(ui, 4, 10, |ui| { ui.label("x"); });
                ui.min_rect().width()
            }).inner;
            assert!(r <= 300.5, "the plate took {r}");
        });
    }

    #[test]
    fn the_vocabulary_paints_without_panicking() {
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(120.0, 40.0));
                plate(ui, r, 3.0);
                gradient_rounded(ui, r, 4, Color32::RED, Color32::BLUE);
                radial_disc(ui, r.center(), 8.0, r.center(), Color32::WHITE, Color32::BLACK);
                let sheet = case(ui, r);
                assert!(sheet.width() < r.width());
                backdrop(ui, r);
                arc(ui, r.center(), 10.0, 0.0, 3.0, Stroke::new(1.0_f32, Color32::WHITE));
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
