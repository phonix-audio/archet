//! Arpeggiator module — reusable, plugin-independent.
//!
//! Features:
//! - 11 algorithmic patterns (Up, Down, UpDown, Random, AsPlayed, Chord, etc.)
//! - 10 stereo placement modes
//! - Programmable rhythm patterns (step sequencer with per-step velocity/gate/rest/tie/accent)
//! - Ratchet (subdivide steps into 2–4 rapid repeats)
//! - Scale quantization (Major, Minor, Pentatonic, Blues, etc.)
//! - Velocity curves (accent, ramp, random, humanize)
//! - Cycle transposition (shift pattern by N semitones each cycle)
//! - Chord strum (speed, direction, alternating)
//! - Pattern chain (sequence multiple rhythm patterns)
//! - Retrigger modes (reset on new note, bar, N beats)
//! - External clock sync (DAW transport)
//! - Swing, latch, gate modes

#![allow(dead_code)]

pub mod event;
pub mod step;
pub mod scale;
pub mod velocity;
pub mod retrigger;
pub mod transpose;
pub mod strum;
pub mod chain;

pub use event::ArpEvent;

/// Instrumentation: total NoteOn events emitted by ANY arpeggiator's `process`.
/// Lets objective tests count arp output (a dropped step shows as a missing
/// increment) instead of unreliable audio-onset detection on repeated pitches.
/// Unconditional (it used to be `#[cfg(test)]`) because the sequencer tests that
/// read it now live in another crate; one relaxed increment per emitted note-on,
/// which happens at step boundaries, never per sample.
pub static NOTE_ON_EMITTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub use step::{StepType, StepData, RhythmPattern};
pub use scale::{ScaleMode, ScaleQuantizer};
pub use velocity::{VelocityCurve, VelocityProcessor};
pub use retrigger::RetriggerMode;
pub use transpose::CycleTranspose;
pub use strum::{StrumDirection, StrumConfig};
pub use chain::PatternChain;

use std::collections::VecDeque;

// ─── Pattern enum ────────────────────────────────────────────────────────────

/// Arpeggiator pattern types
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ArpPattern {
    Up, Down, UpDown, Random, AsPlayed, Chord,
    UpOctave, DownOctave, RandomWalk, RootUp, Converge,
}

impl ArpPattern {
    pub fn from_value(value: f32) -> Self {
        match (value * 10.0) as usize {
            0 => Self::Up, 1 => Self::Down, 2 => Self::UpDown,
            3 => Self::Random, 4 => Self::AsPlayed, 5 => Self::Chord,
            6 => Self::UpOctave, 7 => Self::DownOctave,
            8 => Self::RandomWalk, 9 => Self::RootUp, _ => Self::Converge,
        }
    }

    pub fn to_value(&self) -> f32 {
        match self {
            Self::Up => 0.0, Self::Down => 0.1, Self::UpDown => 0.2,
            Self::Random => 0.3, Self::AsPlayed => 0.4, Self::Chord => 0.5,
            Self::UpOctave => 0.6, Self::DownOctave => 0.7,
            Self::RandomWalk => 0.8, Self::RootUp => 0.9, Self::Converge => 1.0,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Up => "Up", Self::Down => "Down", Self::UpDown => "Up-Down",
            Self::Random => "Random", Self::AsPlayed => "As Played", Self::Chord => "Chord",
            Self::UpOctave => "Up +Octave", Self::DownOctave => "Down +Octave",
            Self::RandomWalk => "Random Walk", Self::RootUp => "Root Up",
            Self::Converge => "Converge",
        }
    }

    pub const COUNT: usize = 11;

    /// Every pattern, in index order (for GUI pickers).
    pub const ALL: [ArpPattern; 11] = [
        Self::Up, Self::Down, Self::UpDown, Self::Random, Self::AsPlayed, Self::Chord,
        Self::UpOctave, Self::DownOctave, Self::RandomWalk, Self::RootUp, Self::Converge,
    ];

    /// Expand a chord into ONE cycle of the arpeggiated note order across
    /// `octaves`, for OFFLINE note generation (generative / compositional tools).
    /// Real-time arpeggiation uses the sample-clocked path; this shares the same
    /// pattern vocabulary so note generators get all 11 patterns for free.
    /// (`Chord` is simultaneous in real time; offline it yields the ascending set.)
    pub fn note_order(self, chord: &[u8], octaves: u8, seed: u64) -> Vec<u8> {
        if chord.is_empty() { return Vec::new(); }
        let octs = octaves.max(1);
        let mut as_played: Vec<u8> = Vec::new();
        for o in 0..octs {
            for &p in chord {
                as_played.push((p as u16 + o as u16 * 12).min(127) as u8);
            }
        }
        let mut sorted = as_played.clone();
        sorted.sort();
        sorted.dedup();
        let n = sorted.len();
        // Small seeded LCG for the random patterns (deterministic per seed).
        let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
        let mut rnd = |m: usize| -> usize {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as usize) % m.max(1)
        };
        match self {
            ArpPattern::Up | ArpPattern::UpOctave => sorted,
            ArpPattern::Down | ArpPattern::DownOctave => { sorted.reverse(); sorted }
            ArpPattern::UpDown => {
                let mut v = sorted.clone();
                if n > 2 {
                    let mut mid = sorted[1..n - 1].to_vec();
                    mid.reverse();
                    v.extend(mid);
                }
                v
            }
            ArpPattern::AsPlayed => as_played,
            ArpPattern::Chord => sorted,
            ArpPattern::Random => (0..n).map(|_| sorted[rnd(n)]).collect(),
            ArpPattern::RandomWalk => {
                // Start low, step +/-1 through the sorted set (clamped).
                let mut idx = 0i32;
                (0..n).map(|_| {
                    let note = sorted[idx as usize];
                    idx = (idx + if rnd(2) == 0 { -1 } else { 1 }).clamp(0, n as i32 - 1);
                    note
                }).collect()
            }
            ArpPattern::RootUp => {
                // Root interleaved with each ascending note: R, s1, R, s2, ...
                let root = sorted[0];
                let mut v = Vec::with_capacity(n * 2);
                for &s in sorted.iter().skip(1) { v.push(root); v.push(s); }
                if v.is_empty() { v.push(root); }
                v
            }
            ArpPattern::Converge => {
                // Outside-in: lowest, highest, 2nd-lowest, 2nd-highest, ...
                let mut v = Vec::with_capacity(n);
                let (mut lo, mut hi) = (0usize, n.saturating_sub(1));
                while lo <= hi {
                    v.push(sorted[lo]);
                    if hi != lo { v.push(sorted[hi]); }
                    lo += 1;
                    if hi == 0 { break; }
                    hi -= 1;
                }
                v
            }
        }
    }
}

// ─── Stereo mode ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StereoMode {
    Mono, Alternate, PingPong, Spread, RandomPan,
    Wide, Rotate, PitchPan, Bounce, Tremolo,
}

impl StereoMode {
    pub fn from_value(value: f32) -> Self {
        match (value * 9.0) as usize {
            0 => Self::Mono, 1 => Self::Alternate, 2 => Self::PingPong,
            3 => Self::Spread, 4 => Self::RandomPan, 5 => Self::Wide,
            6 => Self::Rotate, 7 => Self::PitchPan, 8 => Self::Bounce,
            _ => Self::Tremolo,
        }
    }

    pub fn to_value(&self) -> f32 {
        match self {
            Self::Mono => 0.0, Self::Alternate => 1.0/9.0, Self::PingPong => 2.0/9.0,
            Self::Spread => 3.0/9.0, Self::RandomPan => 4.0/9.0, Self::Wide => 5.0/9.0,
            Self::Rotate => 6.0/9.0, Self::PitchPan => 7.0/9.0, Self::Bounce => 8.0/9.0,
            Self::Tremolo => 1.0,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Mono => "Mono", Self::Alternate => "Alternate", Self::PingPong => "Ping-Pong",
            Self::Spread => "Spread", Self::RandomPan => "Random Pan", Self::Wide => "Wide",
            Self::Rotate => "Rotate", Self::PitchPan => "Pitch Pan", Self::Bounce => "Bounce",
            Self::Tremolo => "Tremolo",
        }
    }

    pub const COUNT: usize = 10;
}

// ─── Gate mode ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateMode {
    Fixed, Tempo, Legato, Staccato,
}

impl GateMode {
    pub fn from_value(value: f32) -> Self {
        match (value * 3.0) as usize {
            0 => Self::Fixed, 1 => Self::Tempo, 2 => Self::Legato, _ => Self::Staccato,
        }
    }

    pub fn to_value(&self) -> f32 {
        match self {
            Self::Fixed => 0.0, Self::Tempo => 1.0/3.0,
            Self::Legato => 2.0/3.0, Self::Staccato => 1.0,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Fixed => "Fixed", Self::Tempo => "Tempo",
            Self::Legato => "Legato", Self::Staccato => "Staccato",
        }
    }
}

// ─── Clock division ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClockDivision {
    ThirtySecond, SixteenthTriplet, SixteenthDotted, Sixteenth,
    EighthTriplet, EighthDotted, Eighth,
    QuarterTriplet, QuarterDotted, Quarter, Half, Whole,
}

impl ClockDivision {
    pub fn from_value(value: f32) -> Self {
        match (value * 11.0) as usize {
            0 => Self::ThirtySecond, 1 => Self::SixteenthTriplet,
            2 => Self::SixteenthDotted, 3 => Self::Sixteenth,
            4 => Self::EighthTriplet, 5 => Self::EighthDotted, 6 => Self::Eighth,
            7 => Self::QuarterTriplet, 8 => Self::QuarterDotted,
            9 => Self::Quarter, 10 => Self::Half, _ => Self::Whole,
        }
    }

    pub fn to_value(&self) -> f32 {
        match self {
            Self::ThirtySecond => 0.0, Self::SixteenthTriplet => 1.0/11.0,
            Self::SixteenthDotted => 2.0/11.0, Self::Sixteenth => 3.0/11.0,
            Self::EighthTriplet => 4.0/11.0, Self::EighthDotted => 5.0/11.0,
            Self::Eighth => 6.0/11.0, Self::QuarterTriplet => 7.0/11.0,
            Self::QuarterDotted => 8.0/11.0, Self::Quarter => 9.0/11.0,
            Self::Half => 10.0/11.0, Self::Whole => 1.0,
        }
    }

    /// Multiplier relative to quarter note (1.0 = quarter)
    pub fn multiplier(&self) -> f32 {
        match self {
            Self::ThirtySecond => 0.125, Self::SixteenthTriplet => 1.0/6.0,
            Self::SixteenthDotted => 0.375, Self::Sixteenth => 0.25,
            Self::EighthTriplet => 1.0/3.0, Self::EighthDotted => 0.75,
            Self::Eighth => 0.5, Self::QuarterTriplet => 2.0/3.0,
            Self::QuarterDotted => 1.5, Self::Quarter => 1.0,
            Self::Half => 2.0, Self::Whole => 4.0,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ThirtySecond => "1/32", Self::SixteenthTriplet => "1/16T",
            Self::SixteenthDotted => "1/16.", Self::Sixteenth => "1/16",
            Self::EighthTriplet => "1/8T", Self::EighthDotted => "1/8.",
            Self::Eighth => "1/8", Self::QuarterTriplet => "1/4T",
            Self::QuarterDotted => "1/4.", Self::Quarter => "1/4",
            Self::Half => "1/2", Self::Whole => "1/1",
        }
    }
}

// ─── Main Arpeggiator ────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct Arpeggiator {
    // Core
    enabled: bool,
    pattern: ArpPattern,
    stereo_mode: StereoMode,
    gate_mode: GateMode,
    clock_division: ClockDivision,
    gate_length: f32,
    octave_range: u8,
    current_octave: i32,
    latch: bool,

    // Note input
    held_notes: Vec<u8>,
    ordered_notes: Vec<u8>,
    played_order: VecDeque<u8>,
    latched_notes: Vec<u8>,

    // Pattern state
    step_index: usize,
    direction_up: bool,
    active_arp_notes: Vec<u8>,

    // Timing
    tempo: f32,
    sample_rate: f32,
    samples_per_step: usize,
    sample_count: usize,
    swing: f32,
    swing_even: bool,

    // External clock
    use_external_clock: bool,
    external_tick_pending: bool,

    // RNG
    random_state: u32,
    last_random_index: usize,

    // Stereo state
    stereo_position: f32,
    last_note: Option<u8>,
    converge_notes: Vec<u8>,
    converge_index: usize,
    converge_direction: bool,

    // ─── New features ───
    /// Rhythm pattern (step sequencer)
    pub rhythm_pattern: RhythmPattern,
    rhythm_enabled: bool,
    rhythm_step: usize,

    /// Scale quantizer
    pub scale: ScaleQuantizer,

    /// Velocity processor
    pub velocity_proc: VelocityProcessor,

    /// Cycle transposition
    pub cycle_transpose: CycleTranspose,
    cycle_count: u32,

    /// Chord strum
    pub strum: StrumConfig,
    strum_queue: Vec<(ArpEvent, usize)>, // (event, samples_until_fire)

    /// Pattern chain
    pub pattern_chain: PatternChain,

    /// Retrigger mode
    retrigger_mode: RetriggerMode,
    beat_counter: f64,

    /// Ratchet state
    ratchet_sub_step: u8,
    ratchet_total: u8,
    ratchet_sample_count: usize,
    ratchet_samples_per_sub: usize,
    ratchet_note: Option<u8>,
    ratchet_velocity: u8,
    ratchet_pan: f32,
}

impl Arpeggiator {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            enabled: false,
            pattern: ArpPattern::Up,
            stereo_mode: StereoMode::Mono,
            gate_mode: GateMode::Fixed,
            clock_division: ClockDivision::Eighth,
            gate_length: 0.8,
            octave_range: 1,
            current_octave: 0,
            latch: false,

            held_notes: Vec::new(),
            ordered_notes: Vec::new(),
            played_order: VecDeque::new(),
            latched_notes: Vec::new(),

            step_index: 0,
            direction_up: true,
            active_arp_notes: Vec::new(),

            tempo: 120.0,
            sample_rate,
            samples_per_step: 22050,
            sample_count: 0,
            swing: 0.0,
            swing_even: true,

            use_external_clock: false,
            external_tick_pending: false,

            random_state: 12345,
            last_random_index: 0,
            stereo_position: 0.0,
            last_note: None,
            converge_notes: Vec::new(),
            converge_index: 0,
            converge_direction: true,

            rhythm_pattern: RhythmPattern::default(),
            rhythm_enabled: false,
            rhythm_step: 0,

            scale: ScaleQuantizer::new(),
            velocity_proc: VelocityProcessor::new(),

            cycle_transpose: CycleTranspose::new(),
            cycle_count: 0,

            strum: StrumConfig::new(),
            strum_queue: Vec::new(),

            pattern_chain: PatternChain::new(),

            retrigger_mode: RetriggerMode::Never,
            beat_counter: 0.0,

            ratchet_sub_step: 0,
            ratchet_total: 1,
            ratchet_sample_count: 0,
            ratchet_samples_per_sub: 0,
            ratchet_note: None,
            ratchet_velocity: 0,
            ratchet_pan: 0.0,
        }
    }

    // ─── Setters (backward-compatible) ───

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled { self.reset(); }
    }
    pub fn is_enabled(&self) -> bool { self.enabled }

    pub fn set_pattern(&mut self, pattern: ArpPattern) {
        self.pattern = pattern;
        self.step_index = 0;
        self.direction_up = true;
    }
    pub fn set_pattern_from_value(&mut self, v: f32) { self.set_pattern(ArpPattern::from_value(v)); }
    pub fn pattern(&self) -> ArpPattern { self.pattern }

    pub fn set_stereo_mode(&mut self, mode: StereoMode) { self.stereo_mode = mode; }
    pub fn set_stereo_mode_from_value(&mut self, v: f32) { self.set_stereo_mode(StereoMode::from_value(v)); }
    pub fn stereo_mode(&self) -> StereoMode { self.stereo_mode }

    pub fn set_gate_mode(&mut self, mode: GateMode) { self.gate_mode = mode; }

    pub fn set_clock_division(&mut self, div: ClockDivision) {
        self.clock_division = div;
        self.update_samples_per_step();
    }
    pub fn set_clock_division_from_value(&mut self, v: f32) { self.set_clock_division(ClockDivision::from_value(v)); }
    pub fn clock_division(&self) -> ClockDivision { self.clock_division }

    pub fn set_gate_length(&mut self, v: f32) { self.gate_length = v.clamp(0.1, 1.0); }
    pub fn gate_length(&self) -> f32 { self.gate_length }

    pub fn set_octave_range(&mut self, r: u8) { self.octave_range = r.clamp(1, 4); }
    pub fn octave_range(&self) -> u8 { self.octave_range }

    pub fn set_tempo(&mut self, bpm: f32) {
        self.tempo = bpm.clamp(20.0, 300.0);
        self.update_samples_per_step();
    }
    pub fn tempo(&self) -> f32 { self.tempo }

    pub fn set_swing(&mut self, v: f32) { self.swing = v.clamp(0.0, 1.0); }
    pub fn swing(&self) -> f32 { self.swing }

    pub fn set_latch(&mut self, v: bool) {
        self.latch = v;
        if !v { self.latched_notes.clear(); }
    }
    pub fn is_latch_enabled(&self) -> bool { self.latch }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.update_samples_per_step();
    }

    // ─── New feature setters ───

    pub fn set_rhythm_enabled(&mut self, v: bool) { self.rhythm_enabled = v; }
    pub fn is_rhythm_enabled(&self) -> bool { self.rhythm_enabled }

    pub fn set_rhythm_pattern(&mut self, p: RhythmPattern) {
        self.rhythm_pattern = p;
        self.rhythm_step = 0;
    }

    pub fn set_rhythm_preset(&mut self, index: usize) {
        self.rhythm_pattern = RhythmPattern::preset(index);
        self.rhythm_step = 0;
    }

    pub fn set_scale_mode(&mut self, mode: ScaleMode) { self.scale.mode = mode; }
    pub fn set_scale_root(&mut self, root: u8) { self.scale.root = root % 12; }

    pub fn set_velocity_curve(&mut self, curve: VelocityCurve) { self.velocity_proc.curve = curve; }
    pub fn set_accent_amount(&mut self, v: u8) { self.velocity_proc.accent_amount = v.min(64); }

    pub fn set_retrigger_mode(&mut self, mode: RetriggerMode) { self.retrigger_mode = mode; }
    pub fn retrigger_mode(&self) -> RetriggerMode { self.retrigger_mode }

    pub fn set_transpose_semitones(&mut self, v: i8) { self.cycle_transpose.semitones = v; }
    pub fn set_transpose_max_cycles(&mut self, v: u8) { self.cycle_transpose.max_cycles = v; }

    pub fn set_strum_speed(&mut self, v: f32) { self.strum.speed = v.clamp(0.0, 1.0); }
    pub fn set_strum_direction(&mut self, d: StrumDirection) { self.strum.direction = d; }

    pub fn set_use_external_clock(&mut self, v: bool) { self.use_external_clock = v; }
    /// Call from DAW transport to advance one step
    pub fn external_tick(&mut self) { self.external_tick_pending = true; }

    pub fn set_chain_enabled(&mut self, v: bool) { self.pattern_chain.enabled = v; }

    // ─── Note input ───

    pub fn note_on(&mut self, note: u8) {
        if !self.held_notes.contains(&note) {
            self.held_notes.push(note);
            self.played_order.push_back(note);

            if self.latch && !self.latched_notes.contains(&note) {
                self.latched_notes.push(note);
            }

            self.update_ordered_notes();

            // Retrigger on new note
            if self.retrigger_mode == RetriggerMode::OnNewNote {
                self.step_index = 0;
                self.rhythm_step = 0;
                self.sample_count = 0;
                self.cycle_transpose.reset();
                self.cycle_count = 0;
            }
        }
    }

    /// Fire the next step on the next call to `process`, from step 0: what
    /// a key going down on an idle arpeggiator does, so the first note
    /// sounds at once instead of up to a step later.
    pub fn fire_now(&mut self) {
        self.step_index = 0;
        self.rhythm_step = 0;
        self.sample_count = self.samples_per_step;
    }

    pub fn note_off(&mut self, note: u8) {
        self.held_notes.retain(|&n| n != note);
        self.played_order.retain(|&n| n != note);
        if !self.latch { self.latched_notes.retain(|&n| n != note); }
        self.update_ordered_notes();
    }

    pub fn all_notes_off(&mut self) {
        self.held_notes.clear();
        self.played_order.clear();
        if !self.latch { self.latched_notes.clear(); }
        self.ordered_notes.clear();
        self.step_index = 0;
        self.rhythm_step = 0;
    }

    /// Hard stop for transport Stop / panic: clears held AND latched notes and
    /// returns the notes that were actively sounding so the caller can release
    /// their voices. Unlike `all_notes_off`, LATCH does not survive this —
    /// audit v2 found latched arps playing forever after Stop (nord's LATCH
    /// even defeated PANIC) because `latched_notes` was kept by design.
    pub fn clear_all(&mut self) -> Vec<u8> {
        self.held_notes.clear();
        self.played_order.clear();
        self.latched_notes.clear();
        self.ordered_notes.clear();
        self.step_index = 0;
        self.rhythm_step = 0;
        let sounding = self.active_arp_notes.clone();
        self.active_arp_notes.clear();
        sounding
    }

    pub fn held_note_count(&self) -> usize { self.held_notes.len() }
    /// True while the arp still has material to play: physically held
    /// keys, LATCH-captured notes, or arp-triggered notes whose gate has
    /// not closed yet. Engines must not idle-skip their audio loop while
    /// this is true, or a latched arp dies as soon as the keys lift.
    pub fn has_notes(&self) -> bool {
        !self.held_notes.is_empty()
            || !self.latched_notes.is_empty()
            || !self.active_arp_notes.is_empty()
    }
    pub fn active_notes(&self) -> &[u8] { &self.active_arp_notes }
    pub fn current_step(&self) -> usize { self.step_index }

    pub fn get_and_clear_active_notes(&mut self) -> Vec<u8> {
        let notes = self.active_arp_notes.clone();
        self.active_arp_notes.clear();
        notes
    }

    pub fn reset(&mut self) {
        self.step_index = 0;
        self.rhythm_step = 0;
        self.direction_up = true;
        self.sample_count = 0;
        self.active_arp_notes.clear();
        self.current_octave = 0;
        self.stereo_position = 0.0;
        self.converge_index = 0;
        self.converge_direction = true;
        self.cycle_count = 0;
        self.cycle_transpose.reset();
        self.strum_queue.clear();
        self.pattern_chain.reset();
        self.ratchet_sub_step = 0;
        self.ratchet_note = None;
        self.beat_counter = 0.0;
    }

    // ─── Internal helpers ───

    fn update_samples_per_step(&mut self) {
        let secs = 60.0 / self.tempo * self.clock_division.multiplier();
        self.samples_per_step = (secs * self.sample_rate) as usize;
    }

    fn update_ordered_notes(&mut self) {
        let notes = if self.latch && self.held_notes.is_empty() {
            &self.latched_notes
        } else {
            &self.held_notes
        };
        self.ordered_notes = notes.iter().copied().collect();
        self.ordered_notes.sort();
        if notes.is_empty() { self.step_index = 0; }
    }

    fn next_random(&mut self) -> u32 {
        self.random_state = self.random_state.wrapping_mul(1103515245).wrapping_add(12345);
        self.random_state
    }

    fn random_float(&mut self) -> f32 {
        (self.next_random() as f32) / (u32::MAX as f32)
    }

    fn get_effective_gate_length(&self) -> f32 {
        match self.gate_mode {
            GateMode::Fixed => self.gate_length,
            GateMode::Tempo => self.gate_length * 0.9,
            GateMode::Legato => (self.gate_length + 0.1).min(1.0),
            GateMode::Staccato => self.gate_length * 0.5,
        }
    }

    fn get_pan(&mut self, note: u8, step: usize) -> f32 {
        match self.stereo_mode {
            StereoMode::Mono => 0.0,
            StereoMode::Alternate => if step % 2 == 0 { -0.7 } else { 0.7 },
            StereoMode::PingPong => if step % 2 == 0 { -1.0 } else { 1.0 },
            StereoMode::Spread => {
                if !self.ordered_notes.is_empty() {
                    let pos = step as f32 / self.ordered_notes.len().max(1) as f32;
                    (pos * 2.0 - 1.0) * 0.8
                } else { 0.0 }
            }
            StereoMode::RandomPan => (self.random_float() * 2.0 - 1.0) * 0.9,
            StereoMode::Wide => {
                let np = (note as f32 - 36.0) / 60.0;
                (np * 2.0 - 1.0).clamp(-1.0, 1.0) * 0.7
            }
            StereoMode::Rotate => {
                self.stereo_position = (self.stereo_position + 0.15) % (2.0 * std::f32::consts::PI);
                self.stereo_position.sin() * 0.8
            }
            StereoMode::PitchPan => {
                let np = (note as f32 - 36.0) / 60.0;
                (np * 2.0 - 1.0).clamp(-1.0, 1.0)
            }
            StereoMode::Bounce => {
                match step % 4 { 0 => -1.0, 2 => 1.0, _ => 0.0 }
            }
            StereoMode::Tremolo => {
                let tp = self.sample_count as f32 / self.samples_per_step as f32;
                (tp * std::f32::consts::PI * 8.0).sin() * 0.6
            }
        }
    }

    fn clamp_note(n: i32) -> u8 { n.clamp(0, 127) as u8 }

    // ─── Note selection (preserved from original) ───

    fn get_next_note(&mut self) -> Option<u8> {
        let notes = if self.latch && self.held_notes.is_empty() {
            if self.latched_notes.is_empty() { return None; }
            &self.latched_notes
        } else if self.held_notes.is_empty() {
            return None;
        } else {
            &self.held_notes
        };
        if notes.is_empty() { return None; }

        let sorted: Vec<u8> = { let mut n: Vec<u8> = notes.iter().copied().collect(); n.sort(); n };

        let note = match self.pattern {
            ArpPattern::Up => {
                let total = sorted.len() * self.octave_range as usize;
                let idx = self.step_index % total;
                let ni = idx % sorted.len();
                let oo = (idx / sorted.len()) as i32;
                Some(Self::clamp_note(sorted[ni] as i32 + oo * 12))
            }
            ArpPattern::Down => {
                let total = sorted.len() * self.octave_range as usize;
                let idx = self.step_index % total;
                let ni = sorted.len() - 1 - (idx % sorted.len());
                let oo = (self.octave_range as i32 - 1) - (idx / sorted.len()) as i32;
                Some(Self::clamp_note(sorted[ni] as i32 + oo * 12))
            }
            ArpPattern::UpDown => {
                let npo = sorted.len();
                let npc = if npo > 1 { npo * 2 - 2 } else { npo } * self.octave_range as usize;
                let cp = self.step_index % npc.max(1);
                let on = if npo > 1 { npo * 2 - 2 } else { npo };
                let _oct = cp / on.max(1);
                let pio = cp % on.max(1);
                let ni = if pio < npo { pio } else { npo * 2 - pio - 2 };
                let base = sorted.get(ni).copied().unwrap_or(sorted[0]);
                Some(Self::clamp_note(base as i32 + (cp / on.max(1)) as i32 * 12))
            }
            ArpPattern::Random => {
                let idx = (self.next_random() as usize) % sorted.len();
                let oo = (self.next_random() as i32 % self.octave_range as i32) * 12;
                Some(Self::clamp_note(sorted[idx] as i32 + oo))
            }
            ArpPattern::AsPlayed => {
                let played: Vec<u8> = self.played_order.iter().copied().collect();
                if played.is_empty() { return None; }
                let total = played.len() * self.octave_range as usize;
                let idx = self.step_index % total;
                let ni = idx % played.len();
                let oo = (idx / played.len()) as i32;
                Some(Self::clamp_note(played[ni] as i32 + oo * 12))
            }
            ArpPattern::Chord => Some(sorted[0]),
            ArpPattern::UpOctave => {
                let total = sorted.len() * self.octave_range as usize;
                let idx = self.step_index % total;
                let ni = idx % sorted.len();
                let oo = (idx / sorted.len()) as i32;
                Some(Self::clamp_note(sorted[ni] as i32 + oo * 12))
            }
            ArpPattern::DownOctave => {
                let total = sorted.len() * self.octave_range as usize;
                let idx = self.step_index % total;
                let ni = sorted.len() - 1 - (idx % sorted.len());
                let oo = (self.octave_range as i32 - 1) - (idx / sorted.len()) as i32;
                Some(Self::clamp_note(sorted[ni] as i32 + oo * 12))
            }
            ArpPattern::RandomWalk => {
                let max_s = 2;
                let delta = ((self.next_random() as i32) % (max_s * 2 + 1)) - max_s;
                self.last_random_index = ((self.last_random_index as i32 + delta).max(0) as usize)
                    .min(sorted.len() - 1);
                let base = sorted[self.last_random_index];
                let oo = (self.next_random() as i32 % self.octave_range as i32) * 12;
                Some(Self::clamp_note(base as i32 + oo))
            }
            ArpPattern::RootUp => {
                let cl = sorted.len() + 1;
                let pos = self.step_index % cl;
                let n = if pos == 0 { sorted[0] } else { sorted[(pos - 1) % sorted.len()] };
                let oo = (self.step_index / cl) as i32 % self.octave_range as i32;
                Some(Self::clamp_note(n as i32 + oo * 12))
            }
            ArpPattern::Converge => {
                if sorted.is_empty() { return None; }
                let center = sorted.len() / 2;
                let cl = sorted.len();
                let pos = self.step_index % (cl * 2);
                let idx = if pos < cl {
                    if pos % 2 == 0 { (pos / 2).min(center) }
                    else { sorted.len() - 1 - (pos / 2) }
                } else {
                    let dp = pos - cl;
                    if dp % 2 == 0 { center + dp / 2 }
                    else { center.saturating_sub(dp / 2 + 1) }
                };
                Some(sorted[idx.min(sorted.len() - 1)])
            }
        };

        note
    }

    /// Compute the total notes in one pattern cycle (for cycle transposition tracking)
    fn notes_per_cycle(&self) -> usize {
        let n = self.ordered_notes.len().max(1);
        match self.pattern {
            ArpPattern::UpDown => {
                if n > 1 { (n * 2 - 2) * self.octave_range as usize }
                else { n * self.octave_range as usize }
            }
            ArpPattern::RootUp => (n + 1) * self.octave_range as usize,
            ArpPattern::Converge => n * 2,
            _ => n * self.octave_range as usize,
        }
    }

    // ─── Main process ───

    /// Process one sample. Returns note events to handle.
    /// Backward-compatible with the original API.
    pub fn process(&mut self, velocity: u8) -> Vec<ArpEvent> {
        let mut events = Vec::new();

        if !self.enabled {
            if !self.active_arp_notes.is_empty() {
                for &note in &self.active_arp_notes {
                    events.push(ArpEvent::note_off(note));
                }
                self.active_arp_notes.clear();
            }
            return events;
        }

        // Get active note set (clone to avoid borrow conflicts with &mut self methods)
        let notes: Vec<u8> = if self.latch && self.held_notes.is_empty() {
            self.latched_notes.clone()
        } else {
            self.held_notes.clone()
        };

        if notes.is_empty() {
            if !self.active_arp_notes.is_empty() {
                for &note in &self.active_arp_notes {
                    events.push(ArpEvent::note_off(note));
                }
                self.active_arp_notes.clear();
            }
            return events;
        }

        // ─── Strum queue: fire delayed chord notes ───
        let mut fired = Vec::new();
        for (i, (evt, delay)) in self.strum_queue.iter_mut().enumerate() {
            if *delay == 0 {
                events.push(*evt);
                self.active_arp_notes.push(evt.note);
                fired.push(i);
            } else {
                *delay -= 1;
            }
        }
        for i in fired.into_iter().rev() {
            self.strum_queue.remove(i);
        }

        // ─── Ratchet: subdivided note repeats within a step ───
        if self.ratchet_total > 1 && self.ratchet_note.is_some() {
            self.ratchet_sample_count += 1;
            if self.ratchet_sample_count >= self.ratchet_samples_per_sub {
                self.ratchet_sample_count = 0;
                self.ratchet_sub_step += 1;
                if self.ratchet_sub_step < self.ratchet_total {
                    // Fire next ratchet note
                    let note = self.ratchet_note.unwrap();
                    // Note off previous
                    for &n in &self.active_arp_notes {
                        events.push(ArpEvent::note_off(n));
                    }
                    self.active_arp_notes.clear();
                    NOTE_ON_EMITTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let mut evt = ArpEvent::note_on(note, self.ratchet_velocity, self.ratchet_pan);
                    evt.ratchet_index = self.ratchet_sub_step;
                    evt.ratchet_count = self.ratchet_total;
                    events.push(evt);
                    self.active_arp_notes.push(note);
                } else {
                    self.ratchet_note = None;
                }
            }
        }

        // ─── Gate off check ───
        let gate_samples = (self.samples_per_step as f32 * self.get_effective_gate_length()) as usize;
        if self.sample_count >= gate_samples && !self.active_arp_notes.is_empty() {
            for &note in &self.active_arp_notes {
                events.push(ArpEvent::note_off(note));
            }
            self.active_arp_notes.clear();
        }

        // ─── Step advance check ───
        let swing_offset = if self.swing_even && self.step_index % 2 == 1 {
            (self.samples_per_step as f32 * self.swing * 0.5) as usize
        } else {
            0
        };

        let should_advance = if self.use_external_clock {
            let tick = self.external_tick_pending;
            self.external_tick_pending = false;
            tick
        } else {
            self.sample_count >= self.samples_per_step + swing_offset
        };

        if should_advance {
            self.sample_count = 0;

            // ─── Beat-based retrigger check ───
            match self.retrigger_mode {
                RetriggerMode::EveryBar => {
                    let beats_per_step = self.clock_division.multiplier();
                    self.beat_counter += beats_per_step as f64;
                    if self.beat_counter >= 4.0 {
                        self.beat_counter = 0.0;
                        self.step_index = 0;
                        self.rhythm_step = 0;
                        self.cycle_transpose.reset();
                        self.cycle_count = 0;
                    }
                }
                RetriggerMode::EveryNBeats(n) => {
                    let beats_per_step = self.clock_division.multiplier();
                    self.beat_counter += beats_per_step as f64;
                    if self.beat_counter >= n as f64 {
                        self.beat_counter = 0.0;
                        self.step_index = 0;
                        self.rhythm_step = 0;
                        self.cycle_transpose.reset();
                        self.cycle_count = 0;
                    }
                }
                _ => {}
            }

            // ─── Get rhythm step data ───
            let active_pattern = if self.pattern_chain.enabled {
                self.pattern_chain.current_pattern().clone()
            } else {
                self.rhythm_pattern.clone()
            };

            let step_data = if self.rhythm_enabled {
                *active_pattern.get_step(self.rhythm_step)
            } else {
                StepData::default() // all Play, no ratchet
            };

            // Advance rhythm step
            if self.rhythm_enabled {
                self.rhythm_step += 1;
                if self.rhythm_step >= active_pattern.step_count as usize {
                    self.rhythm_step = 0;
                    // Pattern cycle complete — advance chain
                    if self.pattern_chain.enabled {
                        self.pattern_chain.advance();
                    }
                }
            }

            // ─── Probability check ───
            if step_data.probability < 1.0 {
                let r = self.random_float();
                if r > step_data.probability {
                    // Skip this step (treat as rest)
                    self.step_index += 1;
                    self.sample_count += 1;
                    return events;
                }
            }

            // ─── Step type handling ───
            match step_data.step_type {
                StepType::Rest => {
                    // Silence: turn off active notes, don't play
                    for &note in &self.active_arp_notes {
                        events.push(ArpEvent::note_off(note));
                    }
                    self.active_arp_notes.clear();
                    self.step_index += 1;
                }
                StepType::Tie => {
                    // Extend previous note — don't retrigger, don't advance note pattern
                    // Just keep current notes playing (skip note-off at gate)
                }
                StepType::Play | StepType::Accent => {
                    let is_accent = step_data.step_type == StepType::Accent;

                    // ─── Cycle transposition tracking ───
                    let npc = self.notes_per_cycle();
                    if npc > 0 && self.step_index > 0 && self.step_index % npc == 0 {
                        self.cycle_count += 1;
                        self.cycle_transpose.advance_cycle();
                    }

                    if self.pattern == ArpPattern::Chord {
                        // ─── Chord mode with strum ───
                        let sorted: Vec<u8> = {
                            let mut n: Vec<u8> = notes.iter().copied().collect();
                            n.sort();
                            n
                        };

                        for (i, &note) in sorted.iter().enumerate() {
                            let note = self.cycle_transpose.apply(note);
                            let note = self.scale.quantize(note);
                            let pan = self.get_pan(note, self.step_index + i);
                            let vel = if step_data.velocity > 0 { step_data.velocity } else { velocity };
                            let vel = self.velocity_proc.process(
                                vel, self.step_index, npc, is_accent, &mut self.random_state,
                            );
                            NOTE_ON_EMITTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let mut evt = ArpEvent::note_on(note, vel, pan);
                            evt.is_accent = is_accent;
                            if step_data.gate_length > 0.0 { evt.gate_length = step_data.gate_length; }

                            let delay = self.strum.note_delay(i, sorted.len(), self.samples_per_step);
                            if delay > 0 {
                                self.strum_queue.push((evt, delay));
                            } else {
                                events.push(evt);
                                self.active_arp_notes.push(note);
                            }
                        }
                        self.strum.advance_alternating();
                    } else if let Some(raw_note) = self.get_next_note() {
                        // ─── Single note with transforms ───
                        let note = self.cycle_transpose.apply(raw_note);
                        let note = self.scale.quantize(note);
                        let pan = self.get_pan(note, self.step_index);
                        let vel = if step_data.velocity > 0 { step_data.velocity } else { velocity };
                        let vel = self.velocity_proc.process(
                            vel, self.step_index, npc, is_accent, &mut self.random_state,
                        );

                        NOTE_ON_EMITTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let mut evt = ArpEvent::note_on(note, vel, pan);
                        evt.is_accent = is_accent;
                        if step_data.gate_length > 0.0 { evt.gate_length = step_data.gate_length; }

                        // ─── Ratchet setup ───
                        if step_data.ratchet > 1 {
                            evt.ratchet_index = 0;
                            evt.ratchet_count = step_data.ratchet;
                            self.ratchet_total = step_data.ratchet;
                            self.ratchet_sub_step = 0;
                            self.ratchet_sample_count = 0;
                            self.ratchet_samples_per_sub = self.samples_per_step / step_data.ratchet as usize;
                            self.ratchet_note = Some(note);
                            self.ratchet_velocity = vel;
                            self.ratchet_pan = pan;
                        } else {
                            self.ratchet_total = 1;
                            self.ratchet_note = None;
                        }

                        events.push(evt);
                        self.active_arp_notes.clear();
                        self.active_arp_notes.push(note);
                    }

                    self.step_index += 1;
                }
            }
        }

        self.sample_count += 1;
        events
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pattern_conversion() {
        for i in 0..=10 {
            let v = i as f32 / 10.0;
            let p = ArpPattern::from_value(v);
            assert!((v - p.to_value()).abs() < 0.15);
        }
    }

    #[test]
    fn test_stereo_mode_conversion() {
        for i in 0..=9 {
            let v = i as f32 / 9.0;
            let m = StereoMode::from_value(v);
            assert!((v - m.to_value()).abs() < 0.15);
        }
    }

    #[test]
    fn test_basic_up_pattern() {
        let mut arp = Arpeggiator::new(44100.0);
        arp.set_enabled(true);
        arp.set_pattern(ArpPattern::Up);
        arp.set_tempo(120.0);
        arp.set_clock_division(ClockDivision::Eighth);
        arp.note_on(60);
        arp.note_on(64);
        arp.note_on(67);

        // Process enough samples to reach the first step boundary
        let mut first_note = None;
        for _ in 0..arp.samples_per_step + 10 {
            let events = arp.process(100);
            for e in &events {
                if e.is_on && first_note.is_none() {
                    first_note = Some(e.note);
                }
            }
        }
        assert_eq!(first_note, Some(60), "First note should be C4");
    }

    #[test]
    fn test_latch_mode() {
        let mut arp = Arpeggiator::new(44100.0);
        arp.set_enabled(true);
        arp.set_latch(true);
        arp.note_on(60);
        arp.note_off(60);
        assert!(arp.latched_notes.contains(&60));
    }

    #[test]
    fn test_scale_quantization() {
        let mut arp = Arpeggiator::new(44100.0);
        arp.set_enabled(true);
        arp.set_pattern(ArpPattern::Up);
        arp.set_scale_mode(ScaleMode::Major);
        arp.set_scale_root(0); // C

        arp.note_on(61); // C# — should quantize to C (60)
        let events = arp.process(100);
        if !events.is_empty() && events[0].is_on {
            assert_eq!(events[0].note, 60); // Quantized to C
        }
    }

    #[test]
    fn test_rhythm_pattern_rest() {
        let mut arp = Arpeggiator::new(44100.0);
        arp.set_enabled(true);
        arp.set_rhythm_enabled(true);
        // Create pattern: Play, Rest, Play, Rest...
        let mut rp = RhythmPattern::new(4);
        rp.steps[0].step_type = StepType::Play;
        rp.steps[1].step_type = StepType::Rest;
        rp.steps[2].step_type = StepType::Play;
        rp.steps[3].step_type = StepType::Rest;
        arp.set_rhythm_pattern(rp);

        arp.note_on(60);

        // Process until we get 2 steps worth
        let mut note_ons = 0;
        for _ in 0..arp.samples_per_step * 4 + 100 {
            let events = arp.process(100);
            for e in &events {
                if e.is_on { note_ons += 1; }
            }
        }
        // Should have 2 note-ons (steps 0 and 2), not 4
        assert_eq!(note_ons, 2, "Expected 2 note-ons from Play/Rest/Play/Rest pattern");
    }

    #[test]
    fn test_ratchet() {
        let mut arp = Arpeggiator::new(44100.0);
        arp.set_enabled(true);
        arp.set_rhythm_enabled(true);
        let mut rp = RhythmPattern::new(1);
        rp.steps[0].ratchet = 3; // 3 subdivisions
        arp.set_rhythm_pattern(rp);

        arp.note_on(60);

        let mut note_ons = 0;
        // Need 2 full steps: first step triggers the note + ratchet, second step's duration
        // lets all ratchet subdivisions fire
        for _ in 0..arp.samples_per_step * 2 + 100 {
            let events = arp.process(100);
            for e in &events {
                if e.is_on { note_ons += 1; }
            }
        }
        // 3 ratchet notes in step 0, plus 3 more in step 1 (pattern loops, step_count=1)
        assert!(note_ons >= 3, "Expected at least 3 note-ons from ratchet=3, got {note_ons}");
    }

    #[test]
    fn test_cycle_transpose() {
        let mut arp = Arpeggiator::new(44100.0);
        arp.set_enabled(true);
        arp.set_pattern(ArpPattern::Up);
        arp.set_transpose_semitones(12); // Octave up each cycle
        arp.set_transpose_max_cycles(3);

        arp.note_on(60); // Single note — cycle = 1 step

        let mut notes_played = Vec::new();
        for _ in 0..arp.samples_per_step * 4 + 100 {
            let events = arp.process(100);
            for e in &events {
                if e.is_on { notes_played.push(e.note); }
            }
        }
        // First note: 60, second: 72 (+12), third: 84 (+24), fourth: 60 (reset)
        assert!(notes_played.len() >= 3);
        assert_eq!(notes_played[0], 60);
        assert_eq!(notes_played[1], 72);
        assert_eq!(notes_played[2], 84);
    }

    #[test]
    fn test_external_clock() {
        let mut arp = Arpeggiator::new(44100.0);
        arp.set_enabled(true);
        arp.set_use_external_clock(true);
        arp.note_on(60);
        arp.note_on(64);

        // No tick — should not advance
        let events = arp.process(100);
        assert!(events.is_empty() || !events.iter().any(|e| e.is_on));

        // Send external tick
        arp.external_tick();
        let events = arp.process(100);
        assert!(events.iter().any(|e| e.is_on));
    }
}
