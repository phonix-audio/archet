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
    /// The notes the instrument plays, lowest and highest: keys outside are
    /// drawn faded, and the accent underlines the span. None marks nothing.
    pub range:         Option<(u8, u8)>,
}

impl Default for KeyboardStyle {
    fn default() -> Self {
        Self {
            accent: ACCENT_SEQ, height: 64.0, labels: true,
            velocity: 100, velocity_by_y: true,
            header: true, wheels: false, id_salt: "kbd", range: None,
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
            ui.label(RichText::new("OCT").color(pal.text_dim).size(pal.type_caption));
            match crate::controls::flow_stepper(ui, 52.0, &format!("{}", state.octave), (style.id_salt, "octave")) {
                Some(-1) => state.octave = state.octave.saturating_sub(1),
                Some(_) if state.octave < 9 => state.octave += 1,
                _ => {}
            }
            let base = state.octave as i32 * 12;
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{}{} .. {}{}",
                note_name((base as u8) % 12), (base / 12) - 1,
                "C", (base / 12) - 1 + state.octaves_shown as i32))
                .color(pal.text_dim).size(pal.type_caption));
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
    // The wheel in its well: a drum shaded round its axis, ribbed, with
    // the accent line across it where it stands.
    let pal = Palette::of(ui.ctx());
    let well = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.bottom() - 14.0));
    crate::theme::recess(ui, well, 3.0);
    let drum = well.shrink2(Vec2::new(2.5, 3.0));
    let mid = drum.center().y;
    let half = drum.height() * 0.5;
    let rib = 3.0;
    let t = (*value - lo) / (hi - lo);
    let phase = (t - 0.5) * drum.height();
    let painter = ui.painter();
    let mut y = drum.top();
    while y < drum.bottom() {
        let u = ((y - mid) / half).clamp(-1.0, 1.0);
        let shade = (1.0 - u * u).sqrt() * (0.55 + 0.45 * (-u).max(0.0));
        let c = crate::theme::lerp_colour(pal.well, crate::theme::lighten(pal.bg_raised, 0.25), shade);
        painter.rect_filled(Rect::from_min_max(Pos2::new(drum.left(), y), Pos2::new(drum.right(), (y + 1.0).min(drum.bottom()))), 0.0, c);
        y += 1.0;
    }
    let mut ry = mid - half - phase.rem_euclid(rib);
    while ry < drum.bottom() {
        if ry > drum.top() {
            painter.line_segment([Pos2::new(drum.left(), ry), Pos2::new(drum.right(), ry)], Stroke::new(1.0_f32, pal.well.gamma_multiply(0.6)));
        }
        ry += rib;
    }
    let my = drum.bottom() - t * drum.height();
    painter.line_segment([Pos2::new(drum.left(), my), Pos2::new(drum.right(), my)], Stroke::new(2.5_f32, accent));
    if snap_back {
        painter.line_segment([Pos2::new(well.left() - 3.0, well.center().y), Pos2::new(well.left() - 0.5, well.center().y)], Stroke::new(1.0_f32, pal.silk_dim));
    }
    crate::theme::printed(ui, Pos2::new(rect.center().x, rect.bottom() - 6.0), label, FontId::proportional(pal.type_caption), pal.text_dim, Align2::CENTER_CENTER);
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
    // The key bed the keys sit in, then ivories shaded from the player's
    // side, then the sharps, each casting its shadow on the ivories below.
    let bed = full_rect.expand2(Vec2::new(3.0, 0.0));
    let painter = ui.painter();
    painter.rect_filled(bed, 2.0, pal.well_edge);
    painter.line_segment([Pos2::new(bed.left(), bed.top() + 0.5), Pos2::new(bed.right(), bed.top() + 0.5)], Stroke::new(2.0_f32, Color32::from_rgb(8, 8, 10)));
    let plays = |note: u8| style.range.is_none_or(|(lo, hi)| (lo..=hi).contains(&note));
    let lower = egui::CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 };
    for key in keys.iter().filter(|k| !k.is_black) {
        let held = state.held.contains(&key.note) || state.active.contains(&key.note);
        let is_hovered = hovered == Some(key.note);
        let is_c = key.note % 12 == 0;
        let ivory = if !plays(key.note) { Color32::from_rgb(132, 134, 136) }
            else if is_c { Color32::from_rgb(214, 216, 212) }
            else { Color32::from_rgb(232, 232, 226) };
        let r = if held { key.rect.translate(Vec2::new(0.0, 1.0)) } else { key.rect };
        let (top, bottom) = if held {
            (style.accent.gamma_multiply(0.8), style.accent)
        } else if is_hovered {
            (crate::theme::lighten(ivory, 0.25), ivory)
        } else {
            (ivory.gamma_multiply(0.86), ivory)
        };
        crate::theme::gradient_rounded(ui, r, lower, top, bottom);
        let p = ui.painter();
        p.rect_stroke(r, lower, Stroke::new(0.8_f32, Color32::from_rgb(70, 70, 74)), egui::StrokeKind::Inside);
        if !held {
            p.line_segment([Pos2::new(r.left() + 2.0, r.bottom() - 1.5), Pos2::new(r.right() - 2.0, r.bottom() - 1.5)], Stroke::new(1.5_f32, Color32::from_rgba_unmultiplied(0, 0, 0, 40)));
        }
        if style.labels && is_c {
            let oct = (key.note / 12).saturating_sub(1);
            p.text(Pos2::new(r.center().x, r.bottom() - 9.0),
                Align2::CENTER_CENTER, format!("C{}", oct), FontId::proportional(pal.type_caption),
                if held { Color32::BLACK } else { Color32::from_rgb(110, 112, 116) });
        }
    }
    for key in keys.iter().filter(|k| k.is_black) {
        let held = state.held.contains(&key.note) || state.active.contains(&key.note);
        let is_hovered = hovered == Some(key.note);
        let p = ui.painter();
        p.rect_filled(key.rect.translate(Vec2::new(1.5, 2.5)).expand(0.5), 2.0, Color32::from_rgba_unmultiplied(0, 0, 0, 90));
        let body = if held { style.accent.gamma_multiply(0.7) }
            else if !plays(key.note) { Color32::from_rgb(52, 52, 56) }
            else { Color32::from_rgb(20, 20, 24) };
        let r = if held { key.rect.translate(Vec2::new(0.0, 1.0)) } else { key.rect };
        crate::theme::gradient_rounded(ui, r, lower, body, crate::theme::lighten(body, 0.06));
        let face = Rect::from_min_max(Pos2::new(r.left() + 1.5, r.top()), Pos2::new(r.right() - 1.5, r.bottom() - r.height() * 0.12));
        let lit = if held { style.accent } else if is_hovered { Color32::from_rgb(70, 72, 80) } else { Color32::from_rgb(44, 45, 52) };
        crate::theme::gradient_rounded(ui, face, lower, crate::theme::lerp_colour(body, lit, 0.4), lit);
        ui.painter().rect_stroke(r, lower, Stroke::new(0.8_f32, Color32::from_rgb(6, 6, 8)), egui::StrokeKind::Inside);
    }
    let painter = ui.painter();
    // The span the instrument plays, underlined in the accent along the
    // bottom edge of its white keys.
    if let Some((lo, hi)) = style.range {
        let span: Vec<&KeyInfo> = keys.iter().filter(|k| !k.is_black && (lo..=hi).contains(&k.note)).collect();
        if let (Some(first), Some(last)) = (span.first(), span.last()) {
            let bar = Rect::from_min_max(
                Pos2::new(first.rect.left(), first.rect.bottom() - 3.0),
                Pos2::new(last.rect.right(), first.rect.bottom()),
            );
            painter.rect_filled(bar, 1u8, style.accent.gamma_multiply(0.85));
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
            id_salt: "snap_kbd", range: Some((48, 84)) };
        let mut harness = Harness::builder()
            .with_size(Vec2::new(560.0, 130.0))
            .build_ui(move |ui| {
                Palette::DEFAULT.install(ui.ctx());
                Palette::DEFAULT.apply_visuals(ui.ctx());
                keyboard_ui(ui, &mut state, &style);
            });
        harness.run_steps(2);
        harness.snapshot("keyboard_widget");
    }
}
