//! The font atlas, filled once.
//!
//! egui rasterises a glyph the first time it is shown and sends the grown
//! atlas to the renderer as a texture update; a page that shows new glyphs
//! at new sizes sends one on the frame it opens. Rasterising every glyph
//! an editor can show, at every size it uses, on its first frame, means no
//! later frame sends a texture at all.

use egui::{Color32, Context, FontId};

/// The printable ASCII an editor can show.
const GLYPHS: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

/// Rasterise every printable ASCII glyph at each font of the style's text
/// styles and at each of `extra`, so the atlas is complete after this
/// frame. Runs once per context: call it at the top of every frame, and
/// the first, when the pixel density is known, is the one that works.
pub fn warm(ctx: &Context, extra: &[FontId]) {
    let once = egui::Id::new("phonix-ui fonts warmed");
    if ctx.data(|d| d.get_temp::<bool>(once)).unwrap_or(false) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(once, true));
    let style = ctx.global_style();
    let mut ids: Vec<FontId> = style.text_styles.values().cloned().collect();
    ids.extend(extra.iter().cloned());
    ids.sort_by(|a, b| a.size.partial_cmp(&b.size).unwrap().then(a.family.to_string().cmp(&b.family.to_string())));
    ids.dedup();
    ctx.fonts_mut(|f| {
        for id in &ids {
            f.layout_no_wrap(GLYPHS.to_owned(), id.clone(), Color32::WHITE);
        }
    });
}

/// Every proportional size in `sizes`, for a `warm` call.
pub fn proportional(sizes: &[f32]) -> Vec<FontId> {
    sizes.iter().map(|s| FontId::proportional(*s)).collect()
}

/// Every monospace size in `sizes`, for a `warm` call.
pub fn monospace(sizes: &[f32]) -> Vec<FontId> {
    sizes.iter().map(|s| FontId::monospace(*s)).collect()
}
