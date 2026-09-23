//! The house faces, and the font atlas, filled once.
//!
//! Every editor sets its text in two faces the SDK carries: Noto Sans
//! SemiCondensed for silkscreen and labels, Noto Sans Mono for readouts,
//! with egui's own faces behind them for any glyph they lack. An instrument
//! may add a display face of its own for its wordmark and headings.
//!
//! egui rasterises a glyph the first time it is shown and sends the grown
//! atlas to the renderer as a texture update; a page that shows new glyphs
//! at new sizes sends one on the frame it opens. Rasterising every glyph
//! an editor can show, at every size it uses, on its first frame, means no
//! later frame sends a texture at all.

use std::sync::Arc;

use egui::{Color32, Context, FontData, FontDefinitions, FontFamily, FontId};

/// The face silkscreen and labels are set in.
pub const SANS: &[u8] = include_bytes!("../assets/fonts/NotoSans-SemiCondensedMedium.ttf");
/// The face readouts are set in: its figures are all one width.
pub const MONO: &[u8] = include_bytes!("../assets/fonts/NotoSansMono-Medium.ttf");

/// The display face an instrument registered, kept so a later `install`
/// does not drop it.
#[derive(Clone, Copy)]
struct Installed {
    display: Option<&'static [u8]>,
    pass: u64,
}

fn installed_id() -> egui::Id {
    egui::Id::new("phonix-ui fonts installed")
}

/// The house faces first in the proportional and monospace families,
/// egui's own after them, and the display family bound to `display`, or to
/// the house face when the instrument has none.
pub fn definitions(display: Option<&'static [u8]>) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("phonix-sans".into(), Arc::new(FontData::from_static(SANS)));
    fonts.font_data.insert("phonix-mono".into(), Arc::new(FontData::from_static(MONO)));
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "phonix-sans".into());
    fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "phonix-mono".into());
    let display_name = match display {
        Some(ttf) => {
            fonts.font_data.insert(crate::theme::DISPLAY.into(), Arc::new(FontData::from_static(ttf)));
            crate::theme::DISPLAY.to_owned()
        }
        None => "phonix-sans".to_owned(),
    };
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let fam = fonts.families.entry(FontFamily::Name(crate::theme::DISPLAY.into())).or_default();
    fam.clear();
    fam.push(display_name);
    fam.extend(fallback.into_iter().filter(|n| n != "phonix-sans"));
    fonts
}

/// Sets the house faces on `ctx`, keeping a display face registered
/// before. Cheap to call every frame: it touches the fonts only the first
/// time, and after `set_display`.
pub fn install(ctx: &Context) {
    if ctx.data(|d| d.get_temp::<Installed>(installed_id())).is_some() {
        return;
    }
    apply(ctx, None);
}

/// Binds `ttf` as the display face, beside the house faces.
pub fn set_display(ctx: &Context, ttf: &'static [u8]) {
    let current = ctx.data(|d| d.get_temp::<Installed>(installed_id()));
    if current.and_then(|c| c.display).map(|d| std::ptr::eq(d, ttf)).unwrap_or(false) {
        return;
    }
    apply(ctx, Some(ttf));
}

fn apply(ctx: &Context, display: Option<&'static [u8]>) {
    ctx.set_fonts(definitions(display));
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(installed_id(), Installed { display, pass }));
}

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
    // Fonts set on this pass take effect on the next: warming now would
    // fill the atlas of the faces about to be replaced.
    if let Some(i) = ctx.data(|d| d.get_temp::<Installed>(installed_id())) {
        if ctx.cumulative_pass_nr() <= i.pass {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The house faces lead their families, egui's follow as fallback, and
    /// the display family is bound with or without an instrument's face.
    #[test]
    fn the_house_faces_lead_and_the_display_family_is_bound() {
        let d = definitions(None);
        assert_eq!(d.families[&FontFamily::Proportional][0], "phonix-sans");
        assert_eq!(d.families[&FontFamily::Monospace][0], "phonix-mono");
        assert!(d.families[&FontFamily::Proportional].len() > 1, "egui's faces stay behind as fallback");
        let disp = &d.families[&FontFamily::Name(crate::theme::DISPLAY.into())];
        assert_eq!(disp[0], "phonix-sans");
        let with = definitions(Some(SANS));
        assert_eq!(with.families[&FontFamily::Name(crate::theme::DISPLAY.into())][0], crate::theme::DISPLAY);
    }

    /// Text set after the first pass is set in the house face, and a
    /// second install is a no-op.
    #[test]
    fn installing_twice_sets_the_fonts_once() {
        let ctx = Context::default();
        let _ = ctx.run(Default::default(), |ctx| install(ctx));
        let first = ctx.data(|d| d.get_temp::<Installed>(installed_id())).unwrap().pass;
        let _ = ctx.run(Default::default(), |ctx| install(ctx));
        assert_eq!(ctx.data(|d| d.get_temp::<Installed>(installed_id())).unwrap().pass, first);
    }
}
