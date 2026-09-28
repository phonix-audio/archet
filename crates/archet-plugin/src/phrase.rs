//! MIDI to bow strokes: which note-on is a new stroke, a slur or a bow
//! change.
//!
//! With the Legato control at Auto, on a bowed patch, soloist or section:
//!
//! - a note-on while exactly one other key is held, pressed alone, slurs
//!   from it: `NoteOnLegato`, the sounding voice, or each player of the
//!   section, carries on to the new pitch and the held key's note-off then
//!   finds no voice;
//! - a note-on with no key held, within `TOUCH_S` of the last key coming
//!   up, is a bow change: `BowChange`, detache without a new attack;
//! - anything else is a new stroke: `NoteOn`. Keys pressed within
//!   `CHORD_S` of each other are a chord, each its own bow.
//!
//! At Off, or on a plucked patch, every note-on is `NoteOn`.
//! The sustain pedal is not read: a bow has no damper, and a pedal holding
//! keys down would turn every following note into a slur.
//!
//! A block's events are handled in time order, and a key that comes up at
//! the same instant another goes down comes up first, whatever order the
//! host sent them in.

use archet::ArchetCommand;

/// Keys pressed within this many seconds of each other are one chord: the
/// spread of a hand striking a chord on a keyboard.
pub const CHORD_S: f32 = 0.030;

/// A key pressed within this many seconds of the last key coming up
/// touches it: a sequencer's back-to-back notes, or a finger change on a
/// keyboard with no audible gap.
pub const TOUCH_S: f32 = 0.010;

/// One MIDI event as the engine takes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Midi {
    On(u8, u8),
    Off(u8),
    /// Bend in semitones.
    Bend(f32),
    /// The modulation wheel, 0 to 1.
    Wheel(f32),
}

impl Midi {
    /// Order among events at one instant: releases first.
    fn rank(self) -> u8 {
        u8::from(!matches!(self, Midi::Off(_)))
    }
}

pub struct Phrasing {
    /// Samples since the stream began, at the current block's first sample.
    now: u64,
    chord: u64,
    touch: u64,
    held: [bool; 128],
    held_count: usize,
    /// When the last key went down.
    last_on: Option<u64>,
    /// When the last held key came up, leaving none held.
    last_up: Option<u64>,
    /// Auto: overlaps slur and touching notes change bow.
    auto: bool,
    /// The patch is bowed, not plucked.
    bowed: bool,
    /// The block's events as (timing, rank, arrival, event); its capacity is
    /// fixed at construction and never grown.
    events: Vec<(u32, u8, u32, Midi)>,
}

impl Phrasing {
    /// A phraser at `sample_rate` holding up to `capacity` events a block
    /// before it hands them on early.
    pub fn new(sample_rate: f32, capacity: usize) -> Self {
        Self {
            now: 0,
            chord: (CHORD_S * sample_rate) as u64,
            touch: (TOUCH_S * sample_rate) as u64,
            held: [false; 128],
            held_count: 0,
            last_on: None,
            last_up: None,
            auto: true,
            bowed: true,
            events: Vec::with_capacity(capacity.max(1)),
        }
    }

    /// The Legato control and whether the patch is bowed.
    pub fn set_mode(&mut self, auto: bool, bowed: bool) {
        self.auto = auto;
        self.bowed = bowed;
    }

    /// Queues one event at `timing` samples into the block. A full queue is
    /// handed on first, so the capacity is never exceeded.
    pub fn push(&mut self, timing: u32, event: Midi, emit: &mut impl FnMut(ArchetCommand)) {
        if self.events.len() == self.events.capacity() {
            self.drain(emit);
        }
        let arrival = self.events.len() as u32;
        self.events.push((timing, event.rank(), arrival, event));
    }

    /// Hands the block's events on in order and moves the clock past the
    /// block's `frames` samples.
    pub fn end_block(&mut self, frames: u32, emit: &mut impl FnMut(ArchetCommand)) {
        self.drain(emit);
        self.now += u64::from(frames);
    }

    fn drain(&mut self, emit: &mut impl FnMut(ArchetCommand)) {
        self.events.sort_unstable_by_key(|&(t, r, a, _)| (t, r, a));
        for i in 0..self.events.len() {
            let (timing, _, _, event) = self.events[i];
            let cmd = self.command(self.now + u64::from(timing), event);
            emit(cmd);
        }
        self.events.clear();
    }

    /// The command one event at sample `t` becomes, and the key state after it.
    fn command(&mut self, t: u64, event: Midi) -> ArchetCommand {
        match event {
            Midi::On(note, vel) => {
                let key = usize::from(note & 0x7f);
                let lone = self.last_on.is_none_or(|on| t.saturating_sub(on) >= self.chord);
                let phrased = self.auto && self.bowed;
                let cmd = if phrased && self.held_count == 1 && !self.held[key] && lone {
                    ArchetCommand::NoteOnLegato(note, vel)
                } else if phrased
                    && self.held_count == 0
                    && self.last_up.is_some_and(|up| t.saturating_sub(up) <= self.touch)
                {
                    ArchetCommand::BowChange(note, vel)
                } else {
                    ArchetCommand::NoteOn(note, vel)
                };
                if !self.held[key] {
                    self.held[key] = true;
                    self.held_count += 1;
                }
                self.last_on = Some(t);
                self.last_up = None;
                cmd
            }
            Midi::Off(note) => {
                let key = usize::from(note & 0x7f);
                if self.held[key] {
                    self.held[key] = false;
                    self.held_count -= 1;
                    if self.held_count == 0 {
                        self.last_up = Some(t);
                    }
                }
                ArchetCommand::NoteOff(note)
            }
            Midi::Bend(semitones) => ArchetCommand::PitchBend(semitones),
            Midi::Wheel(w) => ArchetCommand::ModWheel(w),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use archet::{ArchetEngine, ArchetPatch};
    use std::cell::Cell;
    use std::sync::mpsc;

    const SR: f32 = 48_000.0;
    const BLOCK: u32 = 256;

    /// What a command is, without its payload.
    fn kind(c: &ArchetCommand) -> &'static str {
        match c {
            ArchetCommand::NoteOn(..) => "on",
            ArchetCommand::NoteOnLegato(..) => "slur",
            ArchetCommand::BowChange(..) => "change",
            ArchetCommand::NoteOff(_) => "off",
            _ => "other",
        }
    }

    /// The commands a timed event list becomes, `(seconds, event)`.
    fn phrase(auto: bool, bowed: bool, events: &[(f32, Midi)]) -> Vec<&'static str> {
        let mut p = Phrasing::new(SR, BLOCK as usize);
        p.set_mode(auto, bowed);
        let mut out = Vec::new();
        let end = events.iter().map(|e| (e.0 * SR) as u64).max().unwrap_or(0) + u64::from(BLOCK);
        let mut start = 0u64;
        while start < end {
            for &(s, e) in events {
                let at = (s * SR) as u64;
                if at >= start && at < start + u64::from(BLOCK) {
                    p.push((at - start) as u32, e, &mut |c| out.push(kind(&c)));
                }
            }
            p.end_block(BLOCK, &mut |c| out.push(kind(&c)));
            start += u64::from(BLOCK);
        }
        out
    }

    #[test]
    fn an_overlapping_pair_slurs() {
        let ev = [(0.0, Midi::On(69, 100)), (0.4, Midi::On(71, 100)), (0.45, Midi::Off(69)), (0.8, Midi::Off(71))];
        assert_eq!(phrase(true, true, &ev), ["on", "slur", "off", "off"]);
    }

    #[test]
    fn a_chord_is_separate_bows() {
        let ev = [(0.0, Midi::On(62, 100)), (0.01, Midi::On(69, 100)), (0.02, Midi::On(74, 100)), (0.5, Midi::Off(62)),
                  (0.5, Midi::Off(69)), (0.5, Midi::Off(74))];
        assert_eq!(phrase(true, true, &ev), ["on", "on", "on", "off", "off", "off"]);
    }

    /// A note played against two held keys has no one voice to slur from.
    #[test]
    fn a_note_over_a_held_chord_is_a_new_bow() {
        let ev = [(0.0, Midi::On(62, 100)), (0.0, Midi::On(69, 100)), (0.4, Midi::On(74, 100))];
        assert_eq!(phrase(true, true, &ev), ["on", "on", "on"]);
    }

    /// Back to back at one instant: the release goes first even when the
    /// host sent the next key first, and the next note changes bow.
    #[test]
    fn a_touching_pair_changes_bow() {
        let ev = [(0.0, Midi::On(69, 100)), (0.4, Midi::On(71, 100)), (0.4, Midi::Off(69)), (0.8, Midi::Off(71))];
        assert_eq!(phrase(true, true, &ev), ["on", "off", "change", "off"]);
    }

    #[test]
    fn a_gap_starts_a_new_stroke() {
        let ev = [(0.0, Midi::On(69, 100)), (0.3, Midi::Off(69)), (0.4, Midi::On(71, 100))];
        assert_eq!(phrase(true, true, &ev), ["on", "off", "on"]);
    }

    #[test]
    fn off_and_pluck_reattack() {
        let overlap = [(0.0, Midi::On(69, 100)), (0.4, Midi::On(71, 100)), (0.45, Midi::Off(69))];
        let touch = [(0.0, Midi::On(69, 100)), (0.4, Midi::Off(69)), (0.4, Midi::On(71, 100))];
        assert_eq!(phrase(false, true, &overlap), ["on", "on", "off"]);
        assert_eq!(phrase(false, true, &touch), ["on", "off", "on"]);
        assert_eq!(phrase(true, false, &overlap), ["on", "on", "off"]);
        assert_eq!(phrase(true, false, &touch), ["on", "off", "on"]);
    }

    /// A trill played overlapping slurs every note from the one before.
    #[test]
    fn a_trill_slurs_throughout() {
        let ev = [(0.0, Midi::On(69, 100)), (0.2, Midi::On(71, 100)), (0.22, Midi::Off(69)), (0.4, Midi::On(69, 100)),
                  (0.42, Midi::Off(71)), (0.6, Midi::Off(69))];
        assert_eq!(phrase(true, true, &ev), ["on", "slur", "off", "slur", "off", "off"]);
    }

    /// One event list played into the engine as the plugin plays it, on
    /// the solo violin: the left channel, and the voices the engine's meter
    /// last counted sounding.
    fn render(auto: bool, events: &[(f32, Midi)], secs: f32) -> (Vec<f32>, usize) {
        render_on("Violin Solo", auto, events, secs)
    }

    /// `render` on the factory preset `name`.
    fn render_on(name: &str, auto: bool, events: &[(f32, Midi)], secs: f32) -> (Vec<f32>, usize) {
        let patch = ArchetPatch::factory_presets().into_iter().find(|p| p.name == name).unwrap();
        let (mut eng, tx, mr) = ArchetEngine::new_for_plugin(SR);
        tx.send(ArchetCommand::LoadPatch(Box::new(patch))).unwrap();
        let mut p = Phrasing::new(SR, BLOCK as usize);
        p.set_mode(auto, true);
        let send = |tx: &mpsc::Sender<ArchetCommand>, c: ArchetCommand| tx.send(c).unwrap();
        let mut buf = vec![0.0f32; BLOCK as usize * 2];
        let mut out = Vec::new();
        let mut start = 0u64;
        while (start as f32) < secs * SR {
            for &(s, e) in events {
                let at = (s * SR) as u64;
                if at >= start && at < start + u64::from(BLOCK) {
                    p.push((at - start) as u32, e, &mut |c| send(&tx, c));
                }
            }
            p.end_block(BLOCK, &mut |c| send(&tx, c));
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            out.extend(buf.iter().step_by(2));
            start += u64::from(BLOCK);
        }
        let voices = mr.lock().unwrap().read().map_or(0, |m| m.voice_count);
        (out, voices)
    }

    /// An overlapping pair: the second key down 50 ms before the first
    /// comes up. Rendered to just before that release.
    const OVERLAP: [(f32, Midi); 3] = [(0.0, Midi::On(69, 100)), (0.5, Midi::On(71, 100)), (0.55, Midi::Off(69))];

    /// A touching pair: the second key down as the first comes up.
    const TOUCH: [(f32, Midi); 3] = [(0.0, Midi::On(69, 100)), (0.5, Midi::Off(69)), (0.5, Midi::On(71, 100))];

    /// A slur carries the one voice to the new pitch; Off bows the second
    /// note on a voice of its own, attacked from rest.
    #[test]
    fn a_slur_is_one_voice_and_off_two() {
        assert_eq!(render(true, &OVERLAP, 0.54).1, 1, "the slur started a voice");
        assert_eq!(render(false, &OVERLAP, 0.54).1, 2, "off did not re-attack");
    }

    /// A bow change reverses the sounding voice; Off lets it ring out and
    /// attacks the next note on another.
    #[test]
    fn a_bow_change_is_one_voice_and_off_two() {
        assert_eq!(render(true, &TOUCH, 0.54).1, 1, "the bow change started a voice");
        assert_eq!(render(false, &TOUCH, 0.54).1, 2, "off did not re-attack");
    }

    /// On a section a slur and a bow change keep the one note's players;
    /// Off lights a second set.
    #[test]
    fn a_section_slurs_and_changes_bow_at_auto() {
        for name in ["Violin Section", "Cinematic Strings", "Full String Ensemble"] {
            let (_, one) = render_on(name, true, &[(0.0, Midi::On(69, 100))], 0.54);
            assert!(one > 1, "{name} lit one voice");
            for events in [&OVERLAP, &TOUCH] {
                assert_eq!(render_on(name, true, events, 0.54).1, one, "{name} re-attacked at Auto");
                assert_eq!(render_on(name, false, events, 0.54).1, 2 * one, "{name} did not re-attack at Off");
            }
        }
    }

    /// The Legato control at its two ends, in the contexts it acts in: a
    /// slur and a bow change. The renders differ sample by sample, or the
    /// control is inert.
    ///   cargo test --release -p archet-plugin legato_audit -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn legato_audit() {
        for (name, events) in [("slur", &OVERLAP), ("bow change", &TOUCH)] {
            let (auto, _) = render(true, events, 1.0);
            let (off, _) = render(false, events, 1.0);
            let most = auto.iter().zip(&off).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            println!("  legato in a {name:<10} largest difference {most:.4}");
            assert!(most > 1e-4, "legato is inert in a {name}");
        }
    }

    // -- No allocation on the audio thread -------------------------------

    struct Counting;

    thread_local! {
        static ALLOCS: Cell<usize> = const { Cell::new(0) };
    }

    unsafe impl std::alloc::GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
            ALLOCS.with(|c| c.set(c.get() + 1));
            unsafe { std::alloc::System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
            unsafe { std::alloc::System.dealloc(p, l) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    /// A block of events, overflow included, allocates nothing once the
    /// phraser is built.
    #[test]
    fn phrasing_does_not_allocate() {
        let mut p = Phrasing::new(SR, 8);
        let mut n = 0usize;
        let before = ALLOCS.with(|c| c.get());
        for block in 0..4u32 {
            for i in 0..24u32 {
                let note = (60 + (block * 24 + i) % 24) as u8;
                p.push(i * 10, Midi::On(note, 100), &mut |_| n += 1);
                p.push(i * 10 + 5, Midi::Off(note), &mut |_| n += 1);
            }
            p.push(0, Midi::Bend(0.5), &mut |_| n += 1);
            p.push(0, Midi::Wheel(0.5), &mut |_| n += 1);
            p.end_block(BLOCK, &mut |_| n += 1);
        }
        let after = ALLOCS.with(|c| c.get());
        assert_eq!(n, 4 * 50);
        assert_eq!(after, before, "phrasing allocated {} times", after - before);
    }

    /// The engine renders a section's slurs, bow changes and releases
    /// without allocating once it has played a chord.
    #[test]
    fn slurs_on_a_section_do_not_allocate() {
        let patch = ArchetPatch::factory_presets().into_iter().find(|p| p.name == "Violin Section").unwrap();
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(SR);
        tx.send(ArchetCommand::LoadPatch(Box::new(patch))).unwrap();
        let mut buf = vec![0.0f32; BLOCK as usize * 2];
        let mut play = |eng: &mut ArchetEngine, blocks: usize| {
            let before = ALLOCS.with(|c| c.get());
            for _ in 0..blocks {
                eng.process_audio(&mut buf, 2);
            }
            ALLOCS.with(|c| c.get()) - before
        };
        for n in [62, 66, 69, 74] {
            tx.send(ArchetCommand::NoteOn(n, 90)).unwrap();
        }
        play(&mut eng, 40);
        for n in [62, 66, 69, 74] {
            tx.send(ArchetCommand::NoteOff(n)).unwrap();
        }
        play(&mut eng, 200);
        tx.send(ArchetCommand::NoteOn(69, 90)).unwrap();
        tx.send(ArchetCommand::NoteOn(62, 90)).unwrap();
        play(&mut eng, 40);
        let mut n = 0;
        for cmd in [ArchetCommand::NoteOnLegato(71, 90), ArchetCommand::NoteOff(69), ArchetCommand::NoteOnLegato(64, 90),
                    ArchetCommand::NoteOff(62), ArchetCommand::NoteOff(64), ArchetCommand::NoteOff(71),
                    ArchetCommand::BowChange(72, 90), ArchetCommand::NoteOff(72)] {
            tx.send(cmd).unwrap();
            n += play(&mut eng, 20);
        }
        assert_eq!(n, 0, "the engine allocated {n} times");
    }
}
