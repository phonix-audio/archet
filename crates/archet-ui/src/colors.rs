//! The instrument's palette: spruce and maple, rosin and horsehair.
//!
//! Only what is specific to Archet. The tokens the SHARED widgets read are
//! installed as a `Palette` in `theme`, so the chrome, the knob body and the
//! keyboard look like every other plugin here, and the instrument looks like
//! an instrument sitting inside that frame.
//!
//! The panel is a bow, a string and a body, and it is coloured as those
//! three: the pale horsehair of a bow, the amber of rosin on a string, the
//! warm varnish of a corpus. Downstream of the body there is one signal,
//! and it is the cream trunk every Phonix signal path uses.

use egui::Color32;

// ── The case ─────────────────────────────────────────────────────────

/// The panel, a graphite anodising lit from above.
pub const PANEL_TOP:    Color32 = Color32::from_rgb(56, 53, 50);
pub const PANEL_BOTTOM: Color32 = Color32::from_rgb(32, 30, 29);
/// The folded edge of the extrusion the panel is set into.
pub const PANEL_EDGE:   Color32 = Color32::from_rgb(14, 13, 12);
/// The machined end cheek.
pub const CHEEK_TOP:    Color32 = Color32::from_rgb(102, 98, 90);
pub const CHEEK_BOTTOM: Color32 = Color32::from_rgb(56, 53, 50);
/// A brush line along the cheek.
pub const CHEEK_GRAIN:  Color32 = Color32::from_rgba_premultiplied(10, 9, 8, 44);

/// Silkscreen on the anodising, and its quieter secondary.
pub const SILK:     Color32 = Color32::from_rgb(240, 236, 226);
pub const SILK_DIM: Color32 = Color32::from_rgb(154, 149, 138);

// ── The plates sunk into the panel ───────────────────────────────────

pub const PLATE_TOP:    Color32 = Color32::from_rgb(31, 29, 27);
pub const PLATE_BOTTOM: Color32 = Color32::from_rgb(20, 19, 17);
pub const PLATE_SILK:     Color32 = Color32::from_rgb(234, 230, 220);
pub const PLATE_SILK_DIM: Color32 = Color32::from_rgb(138, 133, 124);
/// A recess cut into a plate: a window, a rail, a switch slot.
pub const WELL:      Color32 = Color32::from_rgb(11, 11, 10);
pub const WELL_EDGE: Color32 = Color32::from_rgb(72, 69, 64);

// ── The three things the instrument is ───────────────────────────────

/// The bow: pale horsehair.
pub const BOW:   Color32 = Color32::from_rgb(232, 224, 196);
/// The rosin that lets it grip, and the string it grips.
pub const ROSIN: Color32 = Color32::from_rgb(226, 148, 62);
/// The corpus: varnished maple.
pub const BODY:  Color32 = Color32::from_rgb(146, 74, 42);
/// The fingerboard, ebony, and the bridge standing on the belly.
pub const EBONY:  Color32 = Color32::from_rgb(38, 34, 32);
pub const BRIDGE: Color32 = Color32::from_rgb(198, 170, 118);

/// A tint at rest: the same colour, two thirds of the way down to the plate
/// it sits on. A lerp rather than a per-channel scale, because a scale with
/// a different offset per channel turns a near-neutral tint a different hue
/// on the way down, and an unlit part has to read as the same part.
pub fn dim(c: Color32) -> Color32 {
    let mix = |a: u8, b: u8| (a as f32 * 0.34 + b as f32 * 0.66) as u8;
    Color32::from_rgb(
        mix(c.r(), PLATE_BOTTOM.r()),
        mix(c.g(), PLATE_BOTTOM.g()),
        mix(c.b(), PLATE_BOTTOM.b()),
    )
}

// ── The one signal ───────────────────────────────────────────────────

/// Downstream of the body there is one path, whatever made it.
pub const TRUNK:     Color32 = Color32::from_rgb(238, 226, 200);
pub const TRUNK_DIM: Color32 = Color32::from_rgb(96, 92, 82);

/// The meter, and the ceiling it must not reach.
pub const METER:     Color32 = Color32::from_rgb(126, 214, 138);
pub const METER_HOT: Color32 = Color32::from_rgb(232, 106, 84);
/// What is clipping, and a lamp with nothing behind it.
pub const CLIP:    Color32 = Color32::from_rgb(232, 106, 84);
pub const LED_OFF: Color32 = Color32::from_rgb(44, 42, 39);

#[cfg(test)]
mod tests {
    use super::*;

    /// Bow, rosin and body are three readings, far enough apart that the eye
    /// can tell which part of the instrument it is looking at.
    #[test]
    fn the_three_parts_are_three_colours() {
        let all = [BOW, ROSIN, BODY];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                let d = (a.r() as i32 - b.r() as i32).abs()
                    + (a.g() as i32 - b.g() as i32).abs()
                    + (a.b() as i32 - b.b() as i32).abs();
                assert!(d > 90, "two parts share a colour: {a:?} and {b:?}");
            }
        }
    }

    /// A tint at rest is darker than the tint alight, and still carries its
    /// hue: an unlit part has to read as the same part, not as a grey one.
    #[test]
    fn a_dim_tint_is_the_same_colour_quieter() {
        for c in [BOW, ROSIN, BODY, TRUNK] {
            let d = dim(c);
            let lum = |x: Color32| x.r() as u32 + x.g() as u32 + x.b() as u32;
            assert!(lum(d) < lum(c), "{c:?} did not go down");
            let brightest = |x: Color32| {
                if x.r() >= x.g() && x.r() >= x.b() { 0 } else if x.g() >= x.b() { 1 } else { 2 }
            };
            assert_eq!(brightest(d), brightest(c), "{c:?} changed hue on the way down");
        }
    }
}
