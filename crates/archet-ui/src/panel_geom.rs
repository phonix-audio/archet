//! Where every part of the instrument is, and what a pixel there means.
//!
//! One place for the numbers the editor has to reason about rather than
//! merely draw, and their inverses, because the bow is drawn across the
//! string on the picture and taken back from it. What the eye sees and what
//! a drag hits are the same numbers, so they are stated once, here.
//!
//! The organising idea is that Archet is three things: a bow on a string, a
//! body the string drives, and a player holding both. The three tiers are
//! those three.

use egui::{Pos2, Rect, Vec2};

// -- The panel's units ------------------------------------------------

pub const KNOB: f32 = 30.0;
pub const KNOB_BIG: f32 = 42.0;
pub const KNOB_SMALL: f32 = 24.0;

/// One knob and the label under it: the column every row is counted in.
pub const CELL: f32 = phonix_ui::widgets::KNOB_GROUP_W;

/// The tiers, and the air between them.
pub const BOW_H: f32 = 188.0;
pub const BODY_H: f32 = 186.0;
pub const PLAYER_H: f32 = 172.0;
pub const TIER_GAP: f32 = 10.0;
pub const MARGIN: f32 = 14.0;

// -- The tiers --------------------------------------------------------

/// The band a tier occupies, inside the panel's margin.
pub fn tier(panel: Rect, index: usize) -> Rect {
    let heights = [BOW_H, BODY_H, PLAYER_H];
    let mut y = panel.top() + MARGIN;
    for h in heights.iter().take(index) {
        y += h + TIER_GAP;
    }
    Rect::from_min_size(
        Pos2::new(panel.left() + MARGIN, y),
        Vec2::new(panel.width() - MARGIN * 2.0, heights[index.min(2)]),
    )
}

// -- The bow on the string --------------------------------------------

/// The window the string is drawn in, from the nut at the left to the
/// bridge at the right.
pub fn string_window(band: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(band.left(), band.top() + 16.0),
        Vec2::new(520.0, BOW_H - 34.0),
    )
}

pub fn string_field(band: Rect) -> Rect {
    string_window(band).shrink(12.0)
}

/// The string runs across the middle of its field.
pub fn string_y(field: Rect) -> f32 {
    field.top() + field.height() * 0.44
}

/// The lowest and highest bow-bridge distance the picture spans, as a
/// fraction of the string's length. The engine's own useful window: a bow
/// lives between about a twentieth and a fifth of the way up from the
/// bridge, and outside that the Helmholtz motion will not start.
pub const BETA_MIN: f32 = 0.02;
pub const BETA_MAX: f32 = 0.30;

/// Where the bow crosses the string, for a bow-bridge distance `beta`. The
/// bridge is at the RIGHT, so a small beta is near it -- sul ponticello --
/// and a large one is out over the fingerboard.
pub fn beta_to_x(field: Rect, beta: f32) -> f32 {
    let t = ((beta.clamp(BETA_MIN, BETA_MAX) - BETA_MIN) / (BETA_MAX - BETA_MIN)).clamp(0.0, 1.0);
    field.right() - t * field.width()
}

/// The inverse, for a bow that has been dragged.
pub fn x_to_beta(field: Rect, x: f32) -> f32 {
    let t = ((field.right() - x) / field.width().max(1.0)).clamp(0.0, 1.0);
    BETA_MIN + t * (BETA_MAX - BETA_MIN)
}

/// Where the knob column starts, from a tier's left edge: the same on the
/// bow and the body, so the two tiers read as one panel.
pub const KNOB_COLUMN: f32 = 552.0;

/// The bow's knobs, to the right of the string: two rows.
pub fn bow_knob(band: Rect, row: usize, i: usize) -> Pos2 {
    Pos2::new(
        band.left() + KNOB_COLUMN + i as f32 * CELL,
        band.top() + 22.0 + row as f32 * 76.0,
    )
}

// -- The body ---------------------------------------------------------

/// The four bodies, stacked down the left of the tier.
pub const RAIL_W: f32 = 128.0;
pub const RAIL_CELL_H: f32 = 34.0;

pub fn body_cell(band: Rect, i: usize) -> Rect {
    Rect::from_min_size(
        Pos2::new(band.left(), band.top() + 18.0 + i as f32 * RAIL_CELL_H),
        Vec2::new(RAIL_W, RAIL_CELL_H - 6.0),
    )
}

/// The window the body's response is drawn in, beside the rail, ending
/// where the bow's window ends a tier above.
pub fn body_window(band: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(band.left() + RAIL_W + 18.0, band.top() + 18.0),
        Vec2::new(KNOB_COLUMN - RAIL_W - 18.0 - 32.0, BODY_H - 36.0),
    )
}

pub fn body_field(band: Rect) -> Rect {
    body_window(band).shrink(8.0)
}

/// The body's controls, to the right of its window, in the bow's column:
/// the switch on the first row, the knob on the second.
pub fn body_knob(band: Rect, row: usize, i: usize) -> Pos2 {
    Pos2::new(
        band.left() + KNOB_COLUMN + i as f32 * CELL,
        band.top() + 20.0 + row as f32 * 76.0,
    )
}

// -- The player -------------------------------------------------------

pub fn player_knob(band: Rect, row: usize, i: usize) -> Pos2 {
    Pos2::new(
        band.left() + i as f32 * CELL,
        band.top() + 20.0 + row as f32 * 62.0,
    )
}

/// The section's own controls sit at the right end, away from the player's.
pub fn section_knob(band: Rect, i: usize) -> Pos2 {
    Pos2::new(band.right() - 3.0 * CELL + i as f32 * CELL, band.top() + 20.0)
}

/// What is leaving, a thin rail under the tier.
pub fn meter(band: Rect) -> Rect {
    Rect::from_min_size(
        Pos2::new(band.left(), band.bottom() - 12.0),
        Vec2::new(band.width(), 6.0),
    )
}

// -- Frequency, and back again ----------------------------------------

/// The band the body's picture spans. A violin's lowest mode sits near
/// 280 Hz and its brilliance band ends near 7 kHz, so the axis is the
/// family's own range and not the ear's.
pub const HZ_MIN: f32 = 60.0;
pub const HZ_MAX: f32 = 10_000.0;

pub fn hz_to_frac(hz: f32) -> f32 {
    let hz = hz.clamp(HZ_MIN, HZ_MAX);
    (hz / HZ_MIN).log2() / (HZ_MAX / HZ_MIN).log2()
}

pub fn frac_to_hz(frac: f32) -> f32 {
    HZ_MIN * (HZ_MAX / HZ_MIN).powf(frac.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(crate::app::W, crate::app::PANEL_H))
    }

    /// The tiers fit the panel the plugin asks a host for.
    #[test]
    fn the_tiers_fit_the_panel() {
        let need = MARGIN * 2.0 + BOW_H + BODY_H + PLAYER_H + TIER_GAP * 2.0;
        assert!(need <= crate::app::PANEL_H, "{need} > {}", crate::app::PANEL_H);
        let last = tier(panel(), 2);
        assert!(last.bottom() <= crate::app::PANEL_H, "the last tier ends at {}", last.bottom());
        assert!(last.right() <= crate::app::W, "the last tier ends at {}", last.right());
    }

    /// Every tier is inside the panel and none overlaps the next.
    #[test]
    fn the_tiers_do_not_overlap() {
        let p = panel();
        for i in 0..2 {
            let a = tier(p, i);
            let b = tier(p, i + 1);
            assert!(a.bottom() <= b.top(), "tier {i} runs into tier {}", i + 1);
            assert!(a.left() >= p.left() && a.right() <= p.right(), "tier {i} is wider than the panel");
        }
    }

    /// Nothing in a tier is drawn outside it, and nothing runs into its
    /// neighbour.
    #[test]
    fn every_part_stays_in_its_tier() {
        let p = panel();
        let bow = tier(p, 0);
        for r in [string_window(bow), string_field(bow)] {
            assert!(bow.contains_rect(r), "{r:?} leaves the bow tier");
        }
        for row in 0..2 {
            for i in 0..4 {
                let at = bow_knob(bow, row, i);
                assert!(at.x + CELL <= bow.right(), "a bow knob runs off the panel");
                assert!(at.y + KNOB + 22.0 <= bow.bottom(), "a bow knob falls out of the tier");
            }
        }
        let body = tier(p, 1);
        for i in 0..4 {
            assert!(body.contains_rect(body_cell(body, i)), "body {i} leaves the tier");
        }
        assert!(body_cell(body, 3).right() < body_window(body).left(), "the rail runs into the window");
        for r in [body_window(body), body_field(body)] {
            assert!(body.contains_rect(r), "{r:?} leaves the body tier");
        }
        assert!(body_window(body).right() < body_knob(body, 0, 0).x, "the window runs into the knobs");
        for row in 0..2 {
            for i in 0..3 {
                let at = body_knob(body, row, i);
                assert!(at.x + CELL <= body.right(), "a body knob runs off the panel");
                assert!(at.y + KNOB + 22.0 <= body.bottom(), "a body knob falls out of the tier");
            }
        }
        let player = tier(p, 2);
        assert!(player.contains_rect(meter(player)));
        for row in 0..2 {
            for i in 0..5 {
                let at = player_knob(player, row, i);
                assert!(at.y + KNOB + 22.0 <= player.bottom(), "a player knob falls out of the tier");
            }
        }
        assert!(player_knob(player, 0, 4).x + CELL <= section_knob(player, 0).x,
                "the player's knobs run into the section's");
    }

    /// The bow's position inverts, so it stays under the hand that dragged
    /// it, and the bridge is at the right: a small beta is sul ponticello.
    #[test]
    fn the_bow_crosses_where_it_is_put() {
        let field = Rect::from_min_size(Pos2::new(20.0, 40.0), Vec2::new(480.0, 100.0));
        for beta in [BETA_MIN, 0.05f32, 0.12, 0.2, BETA_MAX] {
            let back = x_to_beta(field, beta_to_x(field, beta));
            assert!((back - beta).abs() < 1e-4, "{beta} came back as {back}");
        }
        assert!(beta_to_x(field, BETA_MIN) > beta_to_x(field, BETA_MAX),
                "the bridge is not at the right of the picture");
        assert_eq!(x_to_beta(field, field.right() + 100.0), BETA_MIN);
        assert_eq!(x_to_beta(field, field.left() - 100.0), BETA_MAX);
    }

    /// The frequency axis inverts, and an octave is the same distance
    /// wherever it sits.
    #[test]
    fn the_frequency_axis_inverts() {
        for hz in [HZ_MIN, 196.0, 440.0, 2_400.0, HZ_MAX] {
            let back = frac_to_hz(hz_to_frac(hz));
            assert!((back - hz).abs() < hz * 0.001, "{hz} came back as {back}");
        }
        assert_eq!(hz_to_frac(1.0), 0.0);
        assert_eq!(hz_to_frac(48_000.0), 1.0);
        let step = |hz: f32| hz_to_frac(hz * 2.0) - hz_to_frac(hz);
        let low = step(100.0);
        for hz in [200.0f32, 400.0, 800.0, 1600.0] {
            assert!((step(hz) - low).abs() < 1e-5, "an octave at {hz} is not an octave at 100");
        }
    }
}
