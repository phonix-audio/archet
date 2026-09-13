//! How the instrument's surfaces are painted, and the curves the windows
//! draw.
//!
//! Everything is painted rather than loaded. Every surface the editor draws
//! is also a surface it has to reason about: the body's response is drawn
//! from the envelope the engine builds its mode bank against, the bow sits
//! where it is drawn across the string. A bitmap would duplicate that
//! geometry and drift from it.
//!
//! The vocabulary that is not this instrument's, the recess, the plate, the
//! screw, the printed and tracked type, is `phonix_ui::theme`, reading the
//! palette installed here.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Ui};
use phonix_ui::theme::Palette;

use crate::colors::*;

pub use phonix_ui::theme::{gradient_v, heading, lamp, lerp_colour, plate, printed, recess, screw};

/// The colours the shared knobs, meters, pickers and vocabulary read on
/// this instrument.
pub const PALETTE: Palette = Palette {
    bg_dark: WELL,
    bg_panel: PLATE_BOTTOM,
    bg_raised: PANEL_BOTTOM,
    border: PANEL_EDGE,
    text_primary: SILK,
    text_dim: SILK_DIM,
    accent: BOW,
    warm: CLIP,
    ok: METER,
    warn: ROSIN,
    plate_top: PLATE_TOP,
    plate_bottom: PLATE_BOTTOM,
    plate_edge: WELL_EDGE,
    well: WELL,
    well_edge: WELL_EDGE,
    lamp_off: LED_OFF,
    silk: PLATE_SILK,
    silk_dim: PLATE_SILK_DIM,
    pointer: Color32::from_rgb(238, 235, 226),
    pointer_shadow: Color32::from_rgb(16, 15, 14),
};

/// Installs the palette and tunes egui's own widgets to it.
pub fn apply_visuals(ctx: &egui::Context) {
    PALETTE.install(ctx);
    PALETTE.apply_visuals(ctx);
}

/// Tracked capitals, printed.
pub fn tracked(ui: &Ui, left: Pos2, text: &str, font: egui::FontId, ink: Color32, tracking: f32) -> f32 {
    phonix_ui::theme::tracked_text(ui, left, text, font, ink, tracking, phonix_ui::theme::Relief::Printed)
}

/// The anodised panel: a gradient with a soft sheen across the top, which
/// is what a brushed sheet does under a room light.
pub fn panel(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius> + Copy) {
    gradient_v(ui, rect, PANEL_TOP, PANEL_BOTTOM);
    ui.painter().rect_stroke(rect, radius, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
}

/// The machined cheek at each end of the extrusion.
pub fn cheek(ui: &Ui, rect: Rect, radius: impl Into<CornerRadius> + Copy) {
    let _ = radius;
    gradient_v(ui, rect, CHEEK_TOP, CHEEK_BOTTOM);
    let n = (rect.height() / 4.0) as usize;
    for i in 0..n {
        let y = rect.top() + i as f32 * 4.0 + 2.0;
        ui.painter().line_segment(
            [Pos2::new(rect.left() + 2.0, y), Pos2::new(rect.right() - 2.0, y)],
            Stroke::new(1.0_f32, CHEEK_GRAIN),
        );
    }
}

/// A signal run between two parts of the panel: lit when what it carries
/// is sounding, dark when it is not.
pub fn run(ui: &Ui, pts: &[Pos2], live: bool, tint: Color32, dim_tint: Color32) {
    let col = if live { tint } else { dim_tint };
    for w in pts.windows(2) {
        ui.painter().line_segment([w[0], w[1]], Stroke::new(1.6_f32, col));
    }
}

// -- The curves the windows draw --------------------------------------

/// The body's response, as points across a window. The arithmetic is the
/// engine's own (`archet::body::envelope_db`), so a body voiced there moves
/// this picture with it. The per-voice mode jitter is not drawn, because it
/// is per voice: what is drawn is the shape every voice's bank is built
/// against.
pub fn body_curve(win: Rect, inst: usize, bridge_hill_db: f32) -> Vec<Pos2> {
    let n = 128;
    (0..=n)
        .map(|i| {
            let frac = i as f32 / n as f32;
            let hz = crate::panel_geom::frac_to_hz(frac);
            let db = archet::body::envelope_db(inst, bridge_hill_db, hz);
            let y = win.bottom() - ((db + 44.0) / 52.0).clamp(0.0, 1.0) * win.height();
            Pos2::new(win.left() + frac * win.width(), y)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Vec2;

    fn win() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(240.0, 90.0))
    }

    /// Every body draws, and stays inside the window it is drawn in.
    #[test]
    fn every_body_stays_in_its_window() {
        let w = win();
        for inst in 0..4 {
            for hill in [0.0f32, 6.0, 12.0, 20.0] {
                for p in &body_curve(w, inst, hill) {
                    assert!(p.y >= w.top() - 0.01 && p.y <= w.bottom() + 0.01,
                            "body {inst} at hill {hill} left the window at {p:?}");
                }
            }
        }
    }

    /// The bridge hill lifts the band it is named for, and a bigger body
    /// carries it lower: that is the whole difference between a violin and
    /// a cello on this picture.
    #[test]
    fn the_bridge_hill_moves_with_the_body() {
        let at = |pts: &Vec<Pos2>, frac: f32| {
            pts[((frac * (pts.len() - 1) as f32) as usize).min(pts.len() - 1)].y
        };
        let w = win();
        let peak = |inst: usize| {
            let pts = body_curve(w, inst, 12.0);
            let mut best = (0usize, f32::MAX);
            for (i, p) in pts.iter().enumerate() {
                if p.y < best.1 { best = (i, p.y); }
            }
            best.0
        };
        assert!(peak(0) > peak(2), "the cello's hill did not sit below the violin's");
        let quiet = body_curve(w, 0, 5.0);
        let loud = body_curve(w, 0, 20.0);
        let hill = crate::panel_geom::hz_to_frac(2400.0);
        assert!(at(&loud, hill) < at(&quiet, hill), "the hill control did not lift the hill");
    }
}
