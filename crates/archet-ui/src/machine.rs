//! The instrument, drawn.
//!
//! Every window here shows what the engine is doing with the numbers the
//! engine is using, and takes a drag back on the same picture. Nothing is a
//! decoration of a value held elsewhere: the bow crosses the string where
//! the engine's bow-bridge distance puts it, the body's curve is the
//! envelope the engine builds its mode bank against.

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

use crate::colors::*;
use crate::panel_geom as geom;
use crate::theme;

/// The extrusion the panel is set into: two machined cheeks and the
/// anodised sheet between them. Returns the sheet.
pub fn case(ui: &Ui, rect: Rect) -> Rect {
    let cheek_w = 16.0;
    theme::panel(ui, rect, 6.0);
    let left = Rect::from_min_size(rect.min, Vec2::new(cheek_w, rect.height()));
    let right = Rect::from_min_size(Pos2::new(rect.right() - cheek_w, rect.top()), Vec2::new(cheek_w, rect.height()));
    theme::cheek(ui, left, 6.0);
    theme::cheek(ui, right, 6.0);
    for y in [rect.top() + 14.0, rect.bottom() - 14.0] {
        theme::screw(ui, Pos2::new(left.center().x, y), 3.0);
        theme::screw(ui, Pos2::new(right.center().x, y), 3.0);
    }
    Rect::from_min_max(
        Pos2::new(left.right(), rect.top()),
        Pos2::new(right.left(), rect.bottom()),
    )
}

/// The instrument's name, and what it is.
pub fn nameplate(ui: &Ui, rect: Rect, tint: Color32) {
    theme::tracked(ui, Pos2::new(rect.left(), rect.top() + 14.0), "ARCHET",
                   FontId::proportional(19.0), SILK, 3.4);
    theme::tracked(ui, Pos2::new(rect.left() + 148.0, rect.top() + 16.0),
                   "A BOW ON A STRING, THROUGH A BODY",
                   FontId::proportional(8.0), SILK_DIM, 1.6);
    ui.painter().line_segment(
        [Pos2::new(rect.left(), rect.top() + 30.0), Pos2::new(rect.right(), rect.top() + 30.0)],
        Stroke::new(1.0_f32, tint),
    );
}

/// A row of exclusive choices in a slot cut into the plate.
pub fn switch(ui: &mut Ui, rect: Rect, labels: &[&str], sel: usize, tint: Color32, salt: &str) -> Option<usize> {
    theme::recess(ui, rect, 3.0);
    let w = rect.width() / labels.len() as f32;
    let mut picked = None;
    for (i, label) in labels.iter().enumerate() {
        let cell = Rect::from_min_size(
            Pos2::new(rect.left() + i as f32 * w, rect.top()),
            Vec2::new(w, rect.height()),
        );
        let r = ui.interact(cell, ui.id().with((salt, i)), Sense::click());
        if r.clicked() {
            picked = Some(i);
        }
        if i == sel {
            ui.painter().rect_filled(cell.shrink(1.5), 2.0, tint.gamma_multiply(0.28));
            ui.painter().rect_stroke(cell.shrink(1.5), 2.0, Stroke::new(1.0_f32, tint), StrokeKind::Inside);
        }
        theme::printed(ui, cell.center(), label, FontId::proportional(8.5),
                       if i == sel { PLATE_SILK } else { PLATE_SILK_DIM }, Align2::CENTER_CENTER);
    }
    picked
}

/// What a drag on the string changed.
pub struct BowDrag {
    /// Where the bow crosses the string, as the engine's bow-bridge distance.
    pub beta: f32,
    /// How hard it leans on it.
    pub force: f32,
}

/// The string from nut to bridge, with the bow across it where the engine
/// is bowing, and the fingerboard under the far end.
///
/// A drag across moves the bow along the string; a drag down leans on it.
/// When the patch is plucked there is no bow: a plectrum is drawn at the
/// same place, because a pizzicato is still taken at a point.
#[allow(clippy::too_many_arguments)]
pub fn string_window(
    ui: &mut Ui,
    rect: Rect,
    field: Rect,
    beta: f32,
    force: f32,
    speed: f32,
    plucked: bool,
    salt: &str,
) -> Option<BowDrag> {
    theme::recess(ui, rect, 4.0);

    let y = geom::string_y(field);
    // The fingerboard, ending well short of the bridge, and the bridge itself
    // standing on the belly at the right.
    let board = Rect::from_min_max(
        Pos2::new(field.left(), y - 12.0),
        Pos2::new(field.left() + field.width() * 0.55, y + 12.0),
    );
    ui.painter().rect_filled(board, 2.0, EBONY);
    let bridge = Rect::from_min_max(
        Pos2::new(field.right() - 10.0, y - 16.0),
        Pos2::new(field.right() - 2.0, y + 2.0),
    );
    ui.painter().rect_filled(bridge, 1.0, BRIDGE);

    // The string, bright where the bow has rosin on it.
    ui.painter().line_segment(
        [Pos2::new(field.left(), y), Pos2::new(field.right(), y)],
        Stroke::new(2.0_f32, ROSIN),
    );

    let x = geom::beta_to_x(field, beta);
    if plucked {
        // A plectrum: a wedge at the point, and the string pulled aside.
        let pull = 10.0 + 8.0 * force.clamp(0.0, 1.0);
        ui.painter().add(Shape::line(
            vec![Pos2::new(field.left(), y), Pos2::new(x, y - pull), Pos2::new(field.right(), y)],
            Stroke::new(2.0_f32, ROSIN),
        ));
        ui.painter().add(Shape::convex_polygon(
            vec![Pos2::new(x - 5.0, y - pull - 12.0), Pos2::new(x + 5.0, y - pull - 12.0), Pos2::new(x, y - pull)],
            BOW, Stroke::NONE,
        ));
        theme::printed(ui, Pos2::new(field.left(), field.top()), "WHERE THE STRING IS PLUCKED",
                       FontId::proportional(7.5), PLATE_SILK_DIM, Align2::LEFT_TOP);
    } else {
        // The bow: a stick, a ribbon of hair under it, crossing the string.
        // It leans into the string as the force comes up, and it is drawn
        // longer as the bow speed does, because that is what a long stroke is.
        let lean = 6.0 + 14.0 * force.clamp(0.0, 1.0);
        let half = field.height() * (0.16 + 0.22 * speed.clamp(0.0, 1.0));
        let hair = [Pos2::new(x - 5.0, y - half), Pos2::new(x + 5.0, y + half)];
        ui.painter().line_segment(hair, Stroke::new(3.0_f32, BOW));
        let stick = [Pos2::new(x - 5.0 - lean * 0.2, y - half - 6.0),
                     Pos2::new(x + 5.0 + lean * 0.2, y + half + 6.0)];
        ui.painter().line_segment(stick, Stroke::new(1.6_f32, dim(BOW)));
        ui.painter().circle_filled(Pos2::new(x, y), 2.6 + 2.0 * force.clamp(0.0, 1.0), BOW);
        theme::printed(ui, Pos2::new(field.left(), field.top()), "WHERE THE BOW CROSSES THE STRING",
                       FontId::proportional(7.5), PLATE_SILK_DIM, Align2::LEFT_TOP);
    }

    theme::printed(ui, Pos2::new(field.left() + 4.0, field.bottom()), "SUL TASTO",
                   FontId::proportional(7.5), PLATE_SILK_DIM, Align2::LEFT_BOTTOM);
    theme::printed(ui, Pos2::new(field.right() - 4.0, field.bottom()), "SUL PONTICELLO",
                   FontId::proportional(7.5), PLATE_SILK_DIM, Align2::RIGHT_BOTTOM);

    let r = ui.interact(rect, ui.id().with(salt), Sense::drag());
    if r.dragged() {
        if let Some(p) = r.interact_pointer_pos() {
            return Some(BowDrag {
                beta: geom::x_to_beta(field, p.x),
                force: ((p.y - field.top()) / field.height().max(1.0)).clamp(0.0, 1.0),
            });
        }
    }
    None
}

/// The four bodies, stacked. The one that sounds is lit; the others sit
/// unlit, so the rail says what the instrument can be as well as what it is.
pub fn body_rail(ui: &mut Ui, band: Rect, labels: &[&str], sel: usize, salt: &str) -> Option<usize> {
    let mut picked = None;
    for (i, label) in labels.iter().enumerate() {
        let cell = geom::body_cell(band, i);
        let live = i == sel;
        theme::plate(ui, cell, 4.0);
        if live {
            ui.painter().rect_filled(cell, 4.0, BODY.gamma_multiply(0.28));
            ui.painter().rect_stroke(cell, 4.0, Stroke::new(1.4_f32, BODY), StrokeKind::Inside);
        } else {
            ui.painter().rect_stroke(cell, 4.0, Stroke::new(1.0_f32, dim(BODY)), StrokeKind::Inside);
        }
        theme::lamp(ui, Pos2::new(cell.left() + 14.0, cell.center().y), 4.0, live, BODY);
        theme::tracked(ui, Pos2::new(cell.left() + 28.0, cell.center().y + 3.5), label,
                       FontId::proportional(10.0), if live { PLATE_SILK } else { PLATE_SILK_DIM }, 1.8);
        let r = ui.interact(cell, ui.id().with((salt, i)), Sense::click());
        if r.clicked() {
            picked = Some(i);
        }
    }
    picked
}

/// The body's response: the envelope its mode bank is built against, with
/// the bridge hill where the body puts it.
pub fn body_window(ui: &Ui, rect: Rect, field: Rect, inst: usize, hill_db: f32) {
    theme::recess(ui, rect, 4.0);
    for hz in [100.0f32, 1_000.0, 10_000.0] {
        let x = field.left() + geom::hz_to_frac(hz) * field.width();
        ui.painter().line_segment(
            [Pos2::new(x, field.top()), Pos2::new(x, field.bottom())],
            Stroke::new(1.0_f32, WELL_EDGE),
        );
    }
    let pts = theme::body_curve(field, inst, hill_db);
    ui.painter().add(Shape::line(pts, Stroke::new(1.8_f32, BODY)));
    theme::printed(ui, Pos2::new(field.left(), rect.top() + 2.0), "THE CORPUS, AND ITS BRIDGE HILL",
                   FontId::proportional(7.5), PLATE_SILK_DIM, Align2::LEFT_TOP);
}

/// What is leaving, and how close it is to the ceiling.
pub fn meter(ui: &Ui, rect: Rect, level: f32) {
    theme::recess(ui, rect, 2.0);
    let w = rect.width() * level.clamp(0.0, 1.0);
    let col = if level > 0.9 { METER_HOT } else { METER };
    ui.painter().rect_filled(
        Rect::from_min_size(rect.min, Vec2::new(w, rect.height())),
        2.0,
        col,
    );
}
