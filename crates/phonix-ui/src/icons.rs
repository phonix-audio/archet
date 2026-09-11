//! Inline SVG icon library for the sequencer UI.
//!
//! All icons are white-fill SVGs. Use `.tint()` to colorize.
//! Call `install(ctx)` once per frame (it is idempotent).

use egui::{self, Color32, Response, Ui, Vec2};

// ── SVG bytes ─────────────────────────────────────────────────────────────────

pub const PLAY: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <polygon points="4,2 13,8 4,14" fill="white"/>
</svg>"#;

pub const STOP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect x="3" y="3" width="10" height="10" rx="1.5" fill="white"/>
</svg>"#;

pub const REWIND: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect x="2" y="2" width="2.5" height="12" rx="0.75" fill="white"/>
  <polygon points="13,2 6,8 13,14" fill="white"/>
</svg>"#;

pub const RECORD: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <circle cx="8" cy="8" r="5.5" fill="white"/>
</svg>"#;

pub const LOOP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M8,2.5 A5.5,5.5 0 1,1 2.5,8" stroke="white" stroke-width="2"
        fill="none" stroke-linecap="round"/>
  <polygon points="1.5,4.5 5.5,4.5 3.5,8.5" fill="white"/>
</svg>"#;

// Metronome: trapezoidal body + tilted pendulum + base bar. Stylised
// just enough that the silhouette reads even at 16 px without relying
// on any glyph font.
pub const METRONOME: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <polygon points="5,3 11,3 13,14 3,14" stroke="white" stroke-width="1.2"
           fill="none" stroke-linejoin="round"/>
  <line x1="8" y1="13" x2="11" y2="3" stroke="white" stroke-width="1.4" stroke-linecap="round"/>
  <line x1="2" y1="14" x2="14" y2="14" stroke="white" stroke-width="1.4" stroke-linecap="round"/>
</svg>"#;

// Panic / All-notes-off: a bold "!" inside a circle. Reads as
// emergency-stop even at 16 px.
pub const PANIC: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <circle cx="8" cy="8" r="6" stroke="white" stroke-width="1.4" fill="none"/>
  <line x1="8" y1="5" x2="8" y2="10" stroke="white" stroke-width="1.8" stroke-linecap="round"/>
  <circle cx="8" cy="12" r="0.9" fill="white"/>
</svg>"#;

pub const ARROW_UP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M3,11 L8,4 L13,11" stroke="white" stroke-width="2"
        fill="none" stroke-linejoin="round" stroke-linecap="round"/>
</svg>"#;

pub const ARROW_DOWN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M3,5 L8,12 L13,5" stroke="white" stroke-width="2"
        fill="none" stroke-linejoin="round" stroke-linecap="round"/>
</svg>"#;

pub const CLOSE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M4,4 L12,12 M12,4 L4,12"
        stroke="white" stroke-width="2" stroke-linecap="round"/>
</svg>"#;

pub const CHEVRON_LEFT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M10,3 L5,8 L10,13" stroke="white" stroke-width="2"
        fill="none" stroke-linejoin="round" stroke-linecap="round"/>
</svg>"#;

pub const CHEVRON_RIGHT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M6,3 L11,8 L6,13" stroke="white" stroke-width="2"
        fill="none" stroke-linejoin="round" stroke-linecap="round"/>
</svg>"#;

pub const ADD: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M8,3 L8,13 M3,8 L13,8"
        stroke="white" stroke-width="2" stroke-linecap="round"/>
</svg>"#;

pub const ZOOM_IN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <circle cx="7" cy="7" r="4" stroke="white" stroke-width="1.5" fill="none"/>
  <path d="M10.5,10.5 L14,14" stroke="white" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M5,7 L9,7 M7,5 L7,9" stroke="white" stroke-width="1.5" stroke-linecap="round"/>
</svg>"#;

pub const ZOOM_OUT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <circle cx="7" cy="7" r="4" stroke="white" stroke-width="1.5" fill="none"/>
  <path d="M10.5,10.5 L14,14" stroke="white" stroke-width="1.5" stroke-linecap="round"/>
  <path d="M5,7 L9,7" stroke="white" stroke-width="1.5" stroke-linecap="round"/>
</svg>"#;

pub const FIT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect x="2" y="2" width="12" height="12" rx="1" stroke="white" stroke-width="1.5" fill="none"/>
  <path d="M5,8 L11,8 M8,5 L8,11" stroke="white" stroke-width="1.5" stroke-linecap="round"/>
</svg>"#;

// ── Decorative section-header icons ──────────────────────────────────────────

pub const DIAMOND: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M8,2 L14,8 L8,14 L2,8 Z" stroke="white" stroke-width="1.5" fill="none"/>
</svg>"#;

pub const DIAMOND_FILLED: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M8,2 L14,8 L8,14 L2,8 Z" fill="white"/>
</svg>"#;

pub const CIRCLE_DOT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <circle cx="8" cy="8" r="6" stroke="white" stroke-width="1.5" fill="none"/>
  <circle cx="8" cy="8" r="1.8" fill="white"/>
</svg>"#;

pub const CROSS_CIRCLE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <circle cx="8" cy="8" r="6" stroke="white" stroke-width="1.5" fill="none"/>
  <path d="M4,4 L12,12 M12,4 L4,12" stroke="white" stroke-width="1.5"/>
</svg>"#;

pub const BOW_TIE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M3,3 L13,13 M13,3 L3,13" stroke="white" stroke-width="1.5" fill="none"/>
  <path d="M3,3 L3,13 M13,3 L13,13" stroke="white" stroke-width="1.5" fill="none"/>
</svg>"#;

pub const WAVE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M1,5 Q4,1 8,5 T15,5 M1,8 Q4,4 8,8 T15,8 M1,11 Q4,7 8,11 T15,11"
        stroke="white" stroke-width="1.2" fill="none" stroke-linecap="round"/>
</svg>"#;

pub const SINE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M1,8 Q4,1 8,8 T15,8" stroke="white" stroke-width="1.5" fill="none" stroke-linecap="round"/>
</svg>"#;

pub const TRI_DOWN: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M2,4 L14,4 L8,13 Z" stroke="white" stroke-width="1.5" fill="none" stroke-linejoin="round"/>
</svg>"#;

pub const TRI_UP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M8,3 L14,12 L2,12 Z" stroke="white" stroke-width="1.5" fill="none" stroke-linejoin="round"/>
</svg>"#;

pub const SWAP: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M2,5 L12,5 L9,2 M14,11 L4,11 L7,14"
        stroke="white" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"#;

pub const GRID_PLUS: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect x="2" y="2" width="12" height="12" rx="1" stroke="white" stroke-width="1.5" fill="none"/>
  <path d="M2,8 L14,8 M8,2 L8,14" stroke="white" stroke-width="1.2"/>
</svg>"#;

pub const SQUARE_FILLED: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect x="3" y="3" width="10" height="10" rx="1" fill="white"/>
</svg>"#;

pub const SQUARE_OUTLINE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect x="3" y="3" width="10" height="10" rx="1" stroke="white" stroke-width="1.5" fill="none"/>
</svg>"#;

pub const STAR: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M8,1 L9.5,6.5 L15,8 L9.5,9.5 L8,15 L6.5,9.5 L1,8 L6.5,6.5 Z" fill="white"/>
</svg>"#;

pub const BOLT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M9,1 L3,9 L7,9 L6,15 L13,6 L9,6 Z" fill="white"/>
</svg>"#;

pub const HEX: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M5,2 L11,2 L14,8 L11,14 L5,14 L2,8 Z" stroke="white" stroke-width="1.5" fill="none" stroke-linejoin="round"/>
</svg>"#;

pub const ARROW_LR: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M2,8 L14,8 M5,4 L1,8 L5,12 M11,4 L15,8 L11,12"
        stroke="white" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"#;

pub const ARROW_RIGHT: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <path d="M2,8 L13,8 M9,4 L13,8 L9,12"
        stroke="white" stroke-width="1.5" fill="none" stroke-linecap="round" stroke-linejoin="round"/>
</svg>"#;

pub const CAMERA: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect x="1.5" y="4" width="13" height="9" rx="1" stroke="white" stroke-width="1.3" fill="none"/>
  <path d="M5,4 L6,2.5 L10,2.5 L11,4" stroke="white" stroke-width="1.3" fill="none"/>
  <circle cx="8" cy="9" r="2.5" stroke="white" stroke-width="1.3" fill="none"/>
</svg>"#;

pub const DISC: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <circle cx="8" cy="8" r="6" fill="white"/>
</svg>"#;

// ── Loader installation ───────────────────────────────────────────────────────

/// Register egui_extras image loaders with the context.
/// Idempotent — safe to call every frame.
pub fn install(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Draw an icon as an `ImageButton`. `uri` must be unique per icon constant.
///
/// Returns the button [`Response`] — call `.on_hover_text()` on it for tooltips.
pub fn icon_btn(
    ui:   &mut Ui,
    svg:  &'static [u8],
    uri:  &'static str,
    size: f32,
    tint: Color32,
) -> Response {
    let img = egui::Image::from_bytes(uri, svg)
        .fit_to_exact_size(Vec2::splat(size))
        .tint(tint);
    ui.add(egui::Button::image(img))
}

/// Transport-sized icon button (28×28 outer, 16px icon).
pub fn transport_btn(ui: &mut Ui, svg: &'static [u8], uri: &'static str, tint: Color32) -> Response {
    let img = egui::Image::from_bytes(uri, svg)
        .fit_to_exact_size(Vec2::splat(16.0))
        .tint(tint);
    ui.add(
        egui::Button::image(img)
            .frame(true),
    )
}

/// Compact icon button (no frame, 12px icon — for row controls etc.).
pub fn small_icon_btn(ui: &mut Ui, svg: &'static [u8], uri: &'static str, tint: Color32) -> Response {
    let img = egui::Image::from_bytes(uri, svg)
        .fit_to_exact_size(Vec2::splat(12.0))
        .tint(tint);
    ui.add(egui::Button::image(img).frame(false))
}

/// Draw an inline decorative icon (no interaction, no frame) at a given size.
pub fn icon_inline(ui: &mut Ui, svg: &'static [u8], uri: &'static str, size: f32, tint: Color32) {
    let img = egui::Image::from_bytes(uri, svg)
        .fit_to_exact_size(Vec2::splat(size))
        .tint(tint);
    ui.add(img);
}

// ── Unique URIs (one per SVG constant) ───────────────────────────────────────
pub mod uri {
    pub const PLAY:          &str = "bytes://voxseq/play.svg";
    pub const STOP:          &str = "bytes://voxseq/stop.svg";
    pub const REWIND:        &str = "bytes://voxseq/rewind.svg";
    pub const RECORD:        &str = "bytes://voxseq/record.svg";
    pub const LOOP:          &str = "bytes://voxseq/loop.svg";
    pub const METRONOME:     &str = "bytes://voxseq/metronome.svg";
    pub const PANIC:         &str = "bytes://voxseq/panic.svg";
    pub const ARROW_UP:      &str = "bytes://voxseq/arrow_up.svg";
    pub const ARROW_DOWN:    &str = "bytes://voxseq/arrow_down.svg";
    pub const CLOSE:         &str = "bytes://voxseq/close.svg";
    pub const CHEVRON_LEFT:  &str = "bytes://voxseq/chevron_left.svg";
    pub const CHEVRON_RIGHT: &str = "bytes://voxseq/chevron_right.svg";
    pub const ADD:           &str = "bytes://voxseq/add.svg";
    pub const ZOOM_IN:       &str = "bytes://voxseq/zoom_in.svg";
    pub const ZOOM_OUT:      &str = "bytes://voxseq/zoom_out.svg";
    pub const FIT:           &str = "bytes://voxseq/fit.svg";
    pub const DIAMOND:        &str = "bytes://voxseq/diamond.svg";
    pub const DIAMOND_FILLED: &str = "bytes://voxseq/diamond_filled.svg";
    pub const CIRCLE_DOT:     &str = "bytes://voxseq/circle_dot.svg";
    pub const CROSS_CIRCLE:   &str = "bytes://voxseq/cross_circle.svg";
    pub const BOW_TIE:        &str = "bytes://voxseq/bow_tie.svg";
    pub const WAVE:           &str = "bytes://voxseq/wave.svg";
    pub const SINE:           &str = "bytes://voxseq/sine.svg";
    pub const TRI_DOWN:       &str = "bytes://voxseq/tri_down.svg";
    pub const TRI_UP:         &str = "bytes://voxseq/tri_up.svg";
    pub const SWAP:           &str = "bytes://voxseq/swap.svg";
    pub const GRID_PLUS:      &str = "bytes://voxseq/grid_plus.svg";
    pub const SQUARE_FILLED:  &str = "bytes://voxseq/square_filled.svg";
    pub const SQUARE_OUTLINE: &str = "bytes://voxseq/square_outline.svg";
    pub const STAR:           &str = "bytes://voxseq/star.svg";
    pub const BOLT:           &str = "bytes://voxseq/bolt.svg";
    pub const HEX:            &str = "bytes://voxseq/hex.svg";
    pub const ARROW_LR:       &str = "bytes://voxseq/arrow_lr.svg";
    pub const ARROW_RIGHT:    &str = "bytes://voxseq/arrow_right.svg";
    pub const CAMERA:         &str = "bytes://voxseq/camera.svg";
    pub const DISC:           &str = "bytes://voxseq/disc.svg";
}

/// Default icon tint (light gray — visible on dark backgrounds).
pub const DIM: Color32    = Color32::from_rgb(180, 180, 180);
/// Bright white tint.
pub const BRIGHT: Color32 = Color32::WHITE;
/// Active/highlighted tint (blue).
pub const ACTIVE: Color32 = Color32::from_rgb(80, 180, 255);
/// Record active tint (red).
pub const RECORD_ON: Color32 = Color32::from_rgb(240, 60, 60);

/// Look an icon up by NAME, so a declarative layout can ask for one.
///
/// A `.ron` cannot reference a Rust const, and a spec that inlined SVG bytes
/// would stop being a layout description. The table is written out rather than
/// generated because it is the contract between the spec files and this module:
/// renaming a const without updating it here should fail a test, not silently
/// drop an icon.
pub fn by_name(name: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match name {
        "play" => (PLAY, uri::PLAY),
        "stop" => (STOP, uri::STOP),
        "rewind" => (REWIND, uri::REWIND),
        "record" => (RECORD, uri::RECORD),
        "loop" => (LOOP, uri::LOOP),
        "metronome" => (METRONOME, uri::METRONOME),
        "panic" => (PANIC, uri::PANIC),
        "arrow_up" => (ARROW_UP, uri::ARROW_UP),
        "arrow_down" => (ARROW_DOWN, uri::ARROW_DOWN),
        "close" => (CLOSE, uri::CLOSE),
        "chevron_left" => (CHEVRON_LEFT, uri::CHEVRON_LEFT),
        "chevron_right" => (CHEVRON_RIGHT, uri::CHEVRON_RIGHT),
        "add" => (ADD, uri::ADD),
        "zoom_in" => (ZOOM_IN, uri::ZOOM_IN),
        "zoom_out" => (ZOOM_OUT, uri::ZOOM_OUT),
        "fit" => (FIT, uri::FIT),
        "diamond" => (DIAMOND, uri::DIAMOND),
        "diamond_filled" => (DIAMOND_FILLED, uri::DIAMOND_FILLED),
        "circle_dot" => (CIRCLE_DOT, uri::CIRCLE_DOT),
        "cross_circle" => (CROSS_CIRCLE, uri::CROSS_CIRCLE),
        "bow_tie" => (BOW_TIE, uri::BOW_TIE),
        "wave" => (WAVE, uri::WAVE),
        "sine" => (SINE, uri::SINE),
        "tri_down" => (TRI_DOWN, uri::TRI_DOWN),
        "tri_up" => (TRI_UP, uri::TRI_UP),
        "swap" => (SWAP, uri::SWAP),
        "grid_plus" => (GRID_PLUS, uri::GRID_PLUS),
        "square_filled" => (SQUARE_FILLED, uri::SQUARE_FILLED),
        "square_outline" => (SQUARE_OUTLINE, uri::SQUARE_OUTLINE),
        "star" => (STAR, uri::STAR),
        "bolt" => (BOLT, uri::BOLT),
        "hex" => (HEX, uri::HEX),
        "arrow_lr" => (ARROW_LR, uri::ARROW_LR),
        "arrow_right" => (ARROW_RIGHT, uri::ARROW_RIGHT),
        "camera" => (CAMERA, uri::CAMERA),
        "disc" => (DISC, uri::DISC),
        _ => return None,
    })
}

/// Every name `by_name` answers to, for the drift test.
pub const ALL_NAMES: &[&str] = &[
    "play",
    "stop",
    "rewind",
    "record",
    "loop",
    "metronome",
    "panic",
    "arrow_up",
    "arrow_down",
    "close",
    "chevron_left",
    "chevron_right",
    "add",
    "zoom_in",
    "zoom_out",
    "fit",
    "diamond",
    "diamond_filled",
    "circle_dot",
    "cross_circle",
    "bow_tie",
    "wave",
    "sine",
    "tri_down",
    "tri_up",
    "swap",
    "grid_plus",
    "square_filled",
    "square_outline",
    "star",
    "bolt",
    "hex",
    "arrow_lr",
    "arrow_right",
    "camera",
    "disc",
];
