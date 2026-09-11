//! Virtual keyboard for egui
//! ERGO: Realistic piano layout with white + black keys, hover/held visual states.
//!
//! `keyboard_ui` is the ONE shared keyboard used across the whole app: the main
//! window and every plugin editor. It renders a realistic overlapping piano and
//! RETURNS note events (`KeyEvent`), so each host maps them onto its own command
//! channel without the widget knowing anything about that channel. Features:
//! octave shift, click + drag glissando, velocity from vertical click position,
//! hover + held highlighting, active-note (MIDI) reflection, C labels. Notes are
//! played with the mouse or an external MIDI keyboard (no computer-key input).

use crate::colors::*;
use crate::theme::Palette;
use egui::*;

/// A note event emitted by [`keyboard_ui`]. The host maps it to its own command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyEvent {
    On { note: u8, velocity: u8 },
    Off { note: u8 },
    /// Pitch-bend wheel, normalized -1..+1 (0 = center; snaps back on release).
    PitchBend(f32),
    /// Mod wheel, 0..1 (latched).
    ModWheel(f32),
}

/// Persistent state for the shared keyboard widget — one per host app/editor.
#[derive(Clone)]
pub struct KeyboardState {
    /// Notes currently held via the MOUSE (for highlight). The computer keyboard
    /// is no longer a note source — use a MIDI keyboard for playing.
    pub held:          Vec<u8>,
    /// Notes whose KEY IS DOWN at the engine, from MIDI or the sequencer.
    /// The host sets this each frame from its meter; these keys light up like
    /// mouse-held ones. Empty = nothing to mirror.
    ///
    /// Keys down, not voices sounding: a note whose key has been released is
    /// still ringing through its release for as long as a second or two, and
    /// a keyboard that stayed lit through it reads as stuck. An engine
    /// publishes what it is HOLDING.
    pub active:        Vec<u8>,
    /// Leftmost octave: base MIDI note of the keyboard = `octave * 12`.
    pub octave:        u8,
    /// Number of octaves rendered.
    pub octaves_shown: u8,
    /// Note currently under an active mouse drag (drives glissando).
    mouse_note:        Option<u8>,
    /// Pitch-bend wheel position, -1..+1 (snaps back to 0 on release).
    pub pb:            f32,
    /// Mod wheel position, 0..1 (latched).
    pub mw:            f32,
}

impl Default for KeyboardState {
    fn default() -> Self { Self::new(4, 2) }
}

impl KeyboardState {
    /// `octave` is the leftmost octave (base note = octave*12; octave 5 => C at 60).
    pub fn new(octave: u8, octaves_shown: u8) -> Self {
        Self {
            held: Vec::new(),
            active: Vec::new(),
            octave: octave.min(9),
            octaves_shown: octaves_shown.clamp(1, 6),
            mouse_note: None,
            pb: 0.0,
            mw: 0.0,
        }
    }
}

/// Visual + behaviour options for [`keyboard_ui`].
pub struct KeyboardStyle {
    /// Accent colour for held keys.
    pub accent:        Color32,
    /// White-key height in points (black keys are ~60%).
    pub height:        f32,
    /// Draw note-name labels on the C keys.
    pub labels:        bool,
    /// Velocity when the click position doesn't vary it (also the max for by-Y).
    pub velocity:      u8,
    /// Derive velocity from the vertical click position (top soft, bottom loud).
    pub velocity_by_y: bool,
    /// Draw the built-in OCT +/- header. Set false when the host supplies its own
    /// octave control (it must then drive `state.octave` itself).
    pub header:        bool,
    /// Draw pitch-bend + mod wheels on the SAME ROW, left of the keys.
    /// Hosts consume the resulting `KeyEvent::PitchBend` / `ModWheel` events.
    pub wheels:        bool,
    /// Stable id salt so multiple keyboards on one screen don't collide.
    pub id_salt:       &'static str,
}

impl Default for KeyboardStyle {
    fn default() -> Self {
        Self {
            accent: ACCENT_SEQ, height: 64.0, labels: true,
            velocity: 100, velocity_by_y: true,
            header: true, wheels: false, id_salt: "kbd",
        }
    }
}

/// Semitone -> note name (empty for black keys).
fn note_name(semitone: u8) -> &'static str {
    match semitone { 0=>"C",2=>"D",4=>"E",5=>"F",7=>"G",9=>"A",11=>"B",_=>"" }
}

/// White key semitone offsets within an octave
pub const WHITE_KEY_SEMITONES: [u8; 7] = [0, 2, 4, 5, 7, 9, 11]; // C D E F G A B
/// Black key semitone offsets within an octave (sharps)
pub const BLACK_KEY_SEMITONES: [u8; 5] = [1, 3, 6, 8, 10]; // C# D# F# G# A#
/// Black key positions: index into white keys (the black key sits between white[i] and white[i+1])
pub const BLACK_KEY_POSITIONS: [usize; 5] = [0, 1, 3, 4, 5]; // C#=after C, D#=after D, F#=after F, G#=after G, A#=after A


/// The ONE shared keyboard widget. Draws a realistic overlapping piano and
/// returns the note events produced this frame BY THE MOUSE. The host maps each
/// [`KeyEvent`] onto its own command channel. Notes are played with the mouse or
/// an external MIDI keyboard; the computer keyboard is not a note source. Set
/// `state.active` each frame to the notes the engine is HOLDING (see the
/// field) so the keyboard reflects incoming MIDI. `state.held` reflects the
/// widget's mouse-held notes.
pub fn keyboard_ui(ui: &mut Ui, state: &mut KeyboardState, style: &KeyboardStyle) -> Vec<KeyEvent> {
    let mut events: Vec<KeyEvent> = Vec::new();

    // ── Header: octave shift + range readout ──
    if style.header {
        ui.horizontal(|ui| {
            let pal = Palette::of(ui.ctx());
            ui.label(RichText::new("OCT").color(pal.text_dim).size(10.0).strong());
            if ui.small_button("-").clicked() { state.octave = state.octave.saturating_sub(1); }
            ui.label(RichText::new(format!("{}", state.octave)).color(pal.text_primary).size(11.0));
            if ui.small_button("+").clicked() && state.octave < 9 { state.octave += 1; }
            let base = state.octave as i32 * 12;
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{}{} .. {}{}",
                note_name((base as u8) % 12), (base / 12) - 1,
                "C", (base / 12) - 1 + state.octaves_shown as i32))
                .color(pal.text_dim).size(9.0));
        });
    }

    if style.wheels {
        // Wheels + keys share one horizontal row: PB (snap-back) and MW
        // (latched) sit left of the keys at the same height.
        ui.horizontal(|ui| {
            let h = style.height + 2.0;
            let pb = draw_wheel(ui, "PB", &mut state.pb, true, h, style.accent);
            if let Some(v) = pb { events.push(KeyEvent::PitchBend(v)); }
            let mw = draw_wheel(ui, "MW", &mut state.mw, false, h, style.accent);
            if let Some(v) = mw { events.push(KeyEvent::ModWheel(v)); }
            ui.add_space(4.0);
            keyboard_keys(ui, state, style, &mut events);
        });
    } else {
        keyboard_keys(ui, state, style, &mut events);
    }

    events
}

/// One vertical performance wheel. Returns Some(new_value) when it changed
/// this frame. `snap_back` recenters to 0 on release (pitch-bend behaviour).
fn draw_wheel(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    snap_back: bool,
    height: f32,
    accent: Color32,
) -> Option<f32> {
    let (lo, hi) = if snap_back { (-1.0f32, 1.0f32) } else { (0.0f32, 1.0f32) };
    let w = 18.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, height), Sense::click_and_drag());
    let mut changed = None;
    if resp.dragged() || resp.drag_started() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // Top = hi, bottom = lo.
            let t = 1.0 - ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
            let v = lo + t * (hi - lo);
            if (v - *value).abs() > 1e-4 { *value = v; changed = Some(v); }
        }
    }
    if snap_back && resp.drag_stopped() && value.abs() > 1e-4 {
        *value = 0.0;
        changed = Some(0.0);
    }
    // Groove + position marker.
    let painter = ui.painter();
    painter.rect_filled(rect, 3u8, Color32::from_rgb(24, 24, 29));
    let pal = Palette::of(ui.ctx());
    painter.rect_stroke(rect, 3u8, Stroke::new(0.5_f32, pal.border), egui::StrokeKind::Outside);
    let t = (*value - lo) / (hi - lo);
    let y = rect.bottom() - t * rect.height();
    painter.line_segment(
        [Pos2::new(rect.left() + 2.0, y), Pos2::new(rect.right() - 2.0, y)],
        Stroke::new(3.0_f32, accent),
    );
    if snap_back {
        // Center detent line.
        let cy = rect.center().y;
        painter.line_segment(
            [Pos2::new(rect.left(), cy), Pos2::new(rect.right(), cy)],
            Stroke::new(0.5_f32, Color32::from_rgb(70, 72, 80)),
        );
    }
    painter.text(
        Pos2::new(rect.center().x, rect.bottom() - 7.0),
        Align2::CENTER_CENTER, label, FontId::proportional(8.0), pal.text_dim,
    );
    changed
}

/// The key strip itself (rect build + mouse + computer keys + draw). Split out
/// of `keyboard_ui` so the wheels can share its horizontal row.
fn keyboard_keys(ui: &mut Ui, state: &mut KeyboardState, style: &KeyboardStyle, events: &mut Vec<KeyEvent>) {
    let base_note = state.octave as i32 * 12;
    let pal = Palette::of(ui.ctx());
    let num_white = 7 * state.octaves_shown as usize;
    let spacing = 1.5f32;
    let key_width = ((ui.available_width() - spacing * (num_white as f32 - 1.0))
        / num_white as f32).clamp(14.0, 40.0);
    let white_height = style.height;
    let black_height = white_height * 0.6;
    let black_width = key_width * 0.62;

    let (full_rect, response) = ui.allocate_exact_size(
        Vec2::new(key_width * num_white as f32 + spacing * (num_white as f32 - 1.0),
                  white_height + 2.0),
        Sense::click_and_drag(),
    );
    let origin = full_rect.left_top();

    // ── Build key rects (white first, black on top) ──
    struct KeyInfo { rect: Rect, note: u8, is_black: bool }
    let mut keys: Vec<KeyInfo> = Vec::new();
    for oct_idx in 0..state.octaves_shown as usize {
        for (i, &semi) in WHITE_KEY_SEMITONES.iter().enumerate() {
            let key_idx = oct_idx * 7 + i;
            let x = origin.x + key_idx as f32 * (key_width + spacing);
            let note = base_note + oct_idx as i32 * 12 + semi as i32;
            if !(0..=127).contains(&note) { continue; }
            keys.push(KeyInfo {
                rect: Rect::from_min_size(Pos2::new(x, origin.y), Vec2::new(key_width, white_height)),
                note: note as u8, is_black: false });
        }
    }
    for oct_idx in 0..state.octaves_shown as usize {
        for (bi, &pos) in BLACK_KEY_POSITIONS.iter().enumerate() {
            let white_idx = oct_idx * 7 + pos;
            let white_x = origin.x + white_idx as f32 * (key_width + spacing);
            let x = white_x + key_width - black_width / 2.0 + spacing / 2.0;
            let note = base_note + oct_idx as i32 * 12 + BLACK_KEY_SEMITONES[bi] as i32;
            if !(0..=127).contains(&note) { continue; }
            keys.push(KeyInfo {
                rect: Rect::from_min_size(Pos2::new(x, origin.y), Vec2::new(black_width, black_height)),
                note: note as u8, is_black: true });
        }
    }

    // ── Mouse: note under pointer (black keys have priority) ──
    let pointer = ui.input(|i| i.pointer.interact_pos());
    let mut hovered: Option<u8> = None;
    let mut pointer_note: Option<u8> = None;
    let mut pointer_vel = style.velocity;
    if let Some(pos) = pointer {
        for key in keys.iter().rev() {
            if key.rect.contains(pos) {
                hovered = Some(key.note);
                pointer_note = Some(key.note);
                if style.velocity_by_y {
                    let t = ((pos.y - key.rect.top()) / key.rect.height()).clamp(0.0, 1.0);
                    pointer_vel = (40.0 + t * (style.velocity as f32 - 40.0)).round() as u8;
                }
                break;
            }
        }
    }

    // Press / drag glissando / release.
    //
    // The `mouse_note.is_none()` guard must cover BOTH terms. egui promotes a
    // held click into a drag after `max_click_duration` (0.8 s) with no mouse
    // movement at all, so an unguarded `drag_started()` re-fired a second
    // NoteOn for the same key 0.8 s into a press, against a single NoteOff.
    if (response.drag_started() || response.is_pointer_button_down_on())
        && state.mouse_note.is_none()
    {
        if let Some(n) = pointer_note {
            state.mouse_note = Some(n);
            if !state.held.contains(&n) { state.held.push(n); }
            events.push(KeyEvent::On { note: n, velocity: pointer_vel.max(1) });
        }
    } else if response.dragged() {
        // Glissando: the pointer slid onto a different key.
        if state.mouse_note != pointer_note {
            if let Some(prev) = state.mouse_note.take() {
                state.held.retain(|&x| x != prev);
                events.push(KeyEvent::Off { note: prev });
            }
            if let Some(n) = pointer_note {
                state.mouse_note = Some(n);
                if !state.held.contains(&n) { state.held.push(n); }
                events.push(KeyEvent::On { note: n, velocity: pointer_vel.max(1) });
            }
        }
    }
    if response.drag_stopped() || (!response.is_pointer_button_down_on() && state.mouse_note.is_some() && !response.dragged()) {
        if let Some(prev) = state.mouse_note.take() {
            state.held.retain(|&x| x != prev);
            events.push(KeyEvent::Off { note: prev });
        }
    }

    // ── Draw ──
    let painter = ui.painter();
    for key in keys.iter().filter(|k| !k.is_black) {
        // Lit = held with the mouse OR sounding from MIDI/the engine (state.active).
        let held = state.held.contains(&key.note) || state.active.contains(&key.note);
        let is_hovered = hovered == Some(key.note);
        let is_c = key.note % 12 == 0;
        let fill = if held { style.accent }
            else if is_hovered { Color32::from_rgb(200, 210, 220) }
            else if is_c { Color32::from_rgb(182, 192, 202) }
            else { Color32::from_rgb(212, 217, 222) };
        painter.rect_filled(key.rect, 3u8, fill);
        painter.rect_stroke(key.rect, 3u8,
            Stroke::new(if held { 2.0_f32 } else { 0.5_f32 }, if held { Color32::WHITE } else { pal.border }),
            egui::StrokeKind::Outside);
        if style.labels && is_c {
            let oct = (key.note / 12).saturating_sub(1);
            painter.text(Pos2::new(key.rect.center().x, key.rect.bottom() - 9.0),
                Align2::CENTER_CENTER, format!("C{}", oct), FontId::proportional(9.0),
                if held { Color32::BLACK } else { Color32::from_rgb(90, 95, 100) });
        }
    }
    for key in keys.iter().filter(|k| k.is_black) {
        let held = state.held.contains(&key.note) || state.active.contains(&key.note);
        let is_hovered = hovered == Some(key.note);
        let fill = if held { style.accent }
            else if is_hovered { Color32::from_rgb(60, 65, 70) }
            else { Color32::from_rgb(24, 24, 29) };
        painter.rect_filled(key.rect, 2u8, fill);
        painter.rect_stroke(key.rect, 2u8,
            Stroke::new(if held { 1.5_f32 } else { 0.5_f32 }, if held { style.accent } else { Color32::from_rgb(14, 14, 17) }),
            egui::StrokeKind::Outside);
        if !held {
            let hl = Rect::from_min_size(key.rect.left_top(), Vec2::new(key.rect.width(), 2.0));
            painter.rect_filled(hl, 2u8, Color32::from_rgb(48, 50, 56));
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octave_shift_clamps() {
        let mut s = KeyboardState::new(0, 2);
        s.octave = 0;
        // saturating_sub keeps it at 0
        assert_eq!(s.octave.saturating_sub(1), 0);
        s.octave = 9;
        assert!(s.octave <= 9);
    }

    /// egui promotes a held click into a drag after `max_click_duration`
    /// (0.8 s) with no pointer movement at all. A press held longer than that
    /// used to emit a second NoteOn for the same key against a single NoteOff,
    /// which is audible as two notes ~1 s apart and can strand a voice.
    #[test]
    fn held_click_emits_exactly_one_note_on() {
        use egui_kittest::Harness;
        use std::sync::{Arc, Mutex};

        let log: Arc<Mutex<Vec<KeyEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        let mut state = KeyboardState::new(4, 2); // base note 48 (C3)
        let style = KeyboardStyle {
            header: false, wheels: false, labels: false,
            height: 68.0, id_salt: "evt_kbd", ..Default::default()
        };

        let mut harness = Harness::builder()
            .with_size(Vec2::new(400.0, 100.0))
            .build(move |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    let evs = keyboard_ui(ui, &mut state, &style);
                    sink.lock().unwrap().extend(evs);
                });
            });
        harness.run_steps(2);
        log.lock().unwrap().clear(); // ignore anything from layout settling

        // Low on the first white key: below the black keys (0.6 * height), so
        // the hit-test can only land on a white one.
        let pos = Pos2::new(20.0, 68.0);
        harness.input_mut().events.push(Event::PointerMoved(pos));
        harness.input_mut().events.push(Event::PointerButton {
            pos, button: PointerButton::Primary, pressed: true,
            modifiers: Modifiers::default(),
        });
        harness.run_steps(1);

        let after_press = log.lock().unwrap().clone();
        assert_eq!(after_press.len(), 1, "press should emit one NoteOn, got {after_press:?}");
        let note = match after_press[0] {
            KeyEvent::On { note, .. } => note,
            ref e => panic!("expected a NoteOn, got {e:?}"),
        };

        // Hold still, well past egui's 0.8 s click/drag threshold.
        harness.run_steps(90); // ~1.5 s at the default 60 fps step
        let while_held = log.lock().unwrap().clone();
        assert_eq!(while_held.len(), 1,
            "holding a key retriggered it once egui called the press a drag: {while_held:?}");

        harness.input_mut().events.push(Event::PointerButton {
            pos, button: PointerButton::Primary, pressed: false,
            modifiers: Modifiers::default(),
        });
        harness.run_steps(2);

        let all = log.lock().unwrap().clone();
        assert_eq!(all, vec![
            KeyEvent::On { note, velocity: match after_press[0] {
                KeyEvent::On { velocity, .. } => velocity, _ => unreachable!() } },
            KeyEvent::Off { note },
        ], "a press-hold-release must be exactly one On then one Off");
    }


    #[test]
    #[ignore]
    fn keyboard_widget_snapshot() {
        use egui_kittest::Harness;
        let mut state = KeyboardState::new(4, 3);
        state.held = vec![60, 64, 67]; // C major triad highlighted
        let style = KeyboardStyle { accent: ACCENT_SEQ, height: 68.0, labels: true, wheels: false,
            velocity: 110, velocity_by_y: true, header: true,
            id_salt: "snap_kbd" };
        let mut harness = Harness::builder()
            .with_size(Vec2::new(560.0, 130.0))
            .build(move |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    keyboard_ui(ui, &mut state, &style);
                });
            });
        harness.run_steps(2);
        harness.snapshot("keyboard_widget");
    }
}
