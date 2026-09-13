//! ArchetEngine - polyphonic bowed-string voice allocator and audio loop.
//!
//! Mirrors Strata's offline-driving interface (`new_for_plugin`, command queue,
//! interleaved `process_audio`) so the summer_storm render harness drives it the
//! same way as every other engine.

use std::sync::mpsc;

use phonix_rt::{meter_channel, SharedReader, Writer};
use super::patch::{ArchetParam, ArchetPatch};
use super::sympathetic::SympStrings;
use super::voice::{ArchetVoice, MAX_VOICES};

/// Physical voices a unison ever lights, whatever section size is asked
/// for: the number of independent players past which the richness of a
/// section no longer grows to the ear (Ternstroem, eight to twelve), at
/// the low end so that four notes of a section hold on the pool without
/// stealing. A larger section is these voices at the larger section's
/// level. Both `fire_unison` and the voice-sum norm need it: one to spend
/// the voices, the other to divide by what a note actually lights.
const PHYS_CAP: usize = 8;
/// Physical voices a plucked section lights per note.
const PLUCK_CAP: usize = 4;

/// A 32-bit finaliser (MurmurHash3's), so every bit of a small seat
/// number reaches every bit drawn from it.
fn mix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h
}

#[derive(Debug, Clone)]
pub enum ArchetCommand {
    NoteOn(u8, u8),
    /// Same-string legato: retune the currently-bowing voice instead of starting
    /// a fresh bow stroke (slurred notes connect without re-attacking).
    NoteOnLegato(u8, u8),
    /// Detache bow change: reverse the bow on the currently-bowing voice (continuous
    /// contact, no lift -> no pluck) for a separate but connected note.
    BowChange(u8, u8),
    NoteOff(u8),
    AllNotesOff,
    PitchBend(f32),
    SetPolyphony(u8),
    SetOutputLevel(f32),
    LoadPatch(Box<ArchetPatch>),
    /// Set one named field of the patch: what a knob turn sends, where
    /// `LoadPatch` rebuilds the sympathetic strings and re-seeds the pool.
    SetParam { param: ArchetParam, value: f32 },
}

#[derive(Debug, Default, Clone)]
pub struct ArchetMeterState {
    pub peak_l: f32,
    pub peak_r: f32,
    pub voice_count: usize,
    /// Standard field: currently-sounding MIDI notes, for the shared keyboard's
    /// active-note reflection (same field name across every engine).
    pub active_notes: Vec<u8>,
    pub cpu_percent: f32,
    pub patch_snapshot: Option<ArchetPatch>,
}

/// The largest section a patch can ask for: past the voices a note
/// lights, players add nothing the pool can render.
pub const PLAYERS_MAX: f32 = 32.0;

/// The engine's safety: a fader ride on the summed output. The gain is
/// one until the sum passes full scale, then whatever brings the peak back
/// to it, reached over a short slope that a lookahead of the same length
/// hides, so a bridge force's vertical edge is held too; that peak is
/// held for two periods of the lowest string sounding, so the gain never
/// moves inside a cycle, and it comes back at a walking pace once the
/// peak has passed. The factory bank never reaches it; the level control
/// can. Inter-sample overshoot is left to the chain behind the engine.
struct Fader {
    /// The gain applied now.
    gain: f32,
    /// Per-sample share of the way to a lower target the gain moves.
    slope: f32,
    /// The samples in flight, oldest at `at`.
    line: [[f32; 2]; Fader::LOOKAHEAD],
    at: usize,
    /// The highest peak seen since the hold began.
    held: f32,
    /// Samples the held peak is kept before it starts to fall.
    hold_left: usize,
    /// How many samples a new peak is held; set per block from the lowest
    /// string sounding.
    hold: usize,
    /// Per-sample factor the held peak falls by once its hold has run out.
    fall: f32,
}

impl Fader {
    /// The pace the gain comes back at, in dB per second.
    const RETURN_DB_PER_S: f32 = 6.0;
    /// Periods of the lowest string the peak is held for.
    const HOLD_PERIODS: f32 = 2.0;
    /// Samples the output runs behind the gain: the gain's move down is
    /// four time constants long, and lands before the peak leaves the line.
    pub const LOOKAHEAD: usize = 32;

    fn new(sample_rate: f32) -> Self {
        Self {
            gain: 1.0,
            slope: 1.0 - (-4.0 / Self::LOOKAHEAD as f32).exp(),
            line: [[0.0; 2]; Self::LOOKAHEAD],
            at: 0,
            held: 0.0,
            hold_left: 0,
            hold: 0,
            fall: 10f32.powf(-Self::RETURN_DB_PER_S / 20.0 / sample_rate),
        }
    }

    /// Set the hold from the lowest frequency sounding.
    fn hold_for(&mut self, lowest_hz: f32, sample_rate: f32) {
        self.hold = (Self::HOLD_PERIODS * sample_rate / lowest_hz.max(1.0)).ceil() as usize;
    }

    /// A stereo pair in, the pair from `LOOKAHEAD` samples ago out, at the
    /// gain the pair in has moved to.
    fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        let g = self.gain(l.abs().max(r.abs()));
        let out = self.line[self.at];
        self.line[self.at] = [l, r];
        self.at = (self.at + 1) % Self::LOOKAHEAD;
        (out[0] * g, out[1] * g)
    }

    /// The gain after a stereo pair with this peak has entered the line.
    fn gain(&mut self, peak: f32) -> f32 {
        if peak >= self.held {
            self.held = peak;
            self.hold_left = self.hold;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.held *= self.fall;
        }
        let target = if self.held > 1.0 { 1.0 / self.held } else { 1.0 };
        if target < self.gain {
            self.gain += (target - self.gain) * self.slope;
        } else {
            self.gain = target;
        }
        self.gain
    }
}

pub struct ArchetEngine {
    voices: Vec<ArchetVoice>,
    fader: Fader,
    /// The keys that are down. What the keyboard shows: a released note
    /// rings on, and a keyboard lit through its release reads as stuck.
    held_keys: Vec<u8>,
    pub patch: ArchetPatch,
    command_rx: mpsc::Receiver<ArchetCommand>,
    meter_writer: Writer<ArchetMeterState>,
    meter_shadow: ArchetMeterState,
    sample_rate: f32,
    meter_counter: usize,
    peak_l: f32,
    peak_r: f32,
    /// The share of a block's own duration the last blocks took to render,
    /// judged on the worst of the recent past.
    load: f32,
    patch_dirty: bool,
    // Sympathetic open-string bank (the fine-instrument "ring"): persists across
    // notes so runs leave a glowing halo. Rebuilt when the patch instrument changes.
    symp: SympStrings,
    symp_inst: usize,
    /// Seed offset already applied to the voices: re-seeding produces the
    /// same decorrelation for a given offset, and is done only when the
    /// offset changes, so a patch push does not perturb a sounding desk.
    applied_seed: u32,
    symp_level: f32,
    note_seq: u64, // monotonic note-on counter (oldest-voice release)
}

impl ArchetEngine {
    pub fn new(
        sample_rate: f32,
        command_rx: mpsc::Receiver<ArchetCommand>,
        meter_writer: Writer<ArchetMeterState>,
    ) -> Self {
        Self {
            voices: (0..MAX_VOICES).map(|i| ArchetVoice::new(sample_rate, i)).collect(),
            fader: Fader::new(sample_rate),
            held_keys: Vec::new(),
            patch: ArchetPatch::default(),
            command_rx,
            meter_writer,
            meter_shadow: ArchetMeterState::default(),
            sample_rate,
            meter_counter: 0,
            peak_l: 0.0,
            peak_r: 0.0,
            load: 0.0,
            patch_dirty: true,
            symp: SympStrings::new(sample_rate, 0),
            symp_inst: 0,
            applied_seed: 0,
            symp_level: 0.015,
            note_seq: 0,
        }
    }

    /// Follow the host to a new rate. Everything that holds a rate is
    /// rebuilt, so a re-rated engine sounds the same as a fresh one: the
    /// waveguides, the sympathetic strings and the section diffuser are all
    /// tuned in samples.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        if (sample_rate - self.sample_rate).abs() < 1e-3 { return; }
        self.sample_rate = sample_rate;
        self.voices = (0..MAX_VOICES).map(|i| ArchetVoice::new(sample_rate, i)).collect();
        self.fader = Fader::new(sample_rate);
        self.symp = SympStrings::new(sample_rate, self.symp_inst);
        self.held_keys.clear();
        self.patch_dirty = true;
    }

    pub fn new_for_plugin(
        sample_rate: f32,
    ) -> (Self, mpsc::Sender<ArchetCommand>, SharedReader<ArchetMeterState>) {
        let (tx, rx) = mpsc::channel();
        let (mw, mr) = meter_channel::<ArchetMeterState>();
        (Self::new(sample_rate, rx, mw), tx, mr)
    }

    pub fn process_audio(&mut self, output: &mut [f32], channels: usize) {
        // A body ringing down reaches denormal range in every one of its
        // resonators, and a denormal biquad costs fifty times a normal one.
        phonix_rt::denormal::enable_flush_to_zero();
        let started = std::time::Instant::now();
        let frames = output.len() / channels.max(1);
        self.process_commands();

        let poly = self.pool();
        let any_active = self.voices[..poly].iter().any(|v| v.is_active());

        // keep rendering while the sympathetic open strings still ring (the
        // halo must not cut off at rests when all voices go idle)
        let tail_active = self.symp.active();
        if !any_active && !tail_active {
            for s in output.iter_mut() {
                *s = 0.0;
            }
            self.measure_load(started, frames);
            self.publish_meter(frames, 0);
            return;
        }

        // Two terms, and neither is the polyphony setting, which is how much
        // overlap the player is allowed, not how much sounds.
        //
        // The first is a fixed headroom for the notes a player sounds at
        // once, the chord the instrument is voiced to hold; it cannot follow
        // the live count, which would make a desk pump as notes enter and
        // leave.
        //
        // The second is the microphone. A section is as loud as its
        // players' incoherent sum, the root of their number (Meyer), at a
        // fixed distance; but a section is recorded from as far as it is
        // wide, and a desk's width grows with the root of its players too,
        // so a section note reaches the microphone at a soloist's level and
        // brings its players as density. The voices that render a note sum
        // incoherently, and dividing by the root of their number is that.
        const CHORD: f32 = 8.0;
        let lit = self.voices_per_note();
        let norm = self.patch.output_level / (CHORD * lit as f32).sqrt();
        // The open strings a plucked note occupies right now cannot ring in
        // sympathy: they are the strings sounding, under a finger or plucked.
        let mut held = [false; 4];
        let mut key_down = false;
        for v in self.voices[..poly].iter() {
            if let Some((inst, string)) = v.on_string() {
                if v.is_active() && inst == self.symp_inst && string < 4 {
                    held[string] = true;
                }
                key_down |= v.note.is_some();
            }
        }
        // With no key down on a plucked patch the hand rests on the strings
        // and nothing rings in sympathy; a held note or chord keeps its halo.
        if self.patch.pluck && !key_down {
            held = [true; 4];
        }
        self.symp.set_held(held);
        let lowest = self.voices[..poly]
            .iter()
            .filter(|v| v.is_active())
            .map(|v| v.frequency())
            .fold(f32::INFINITY, f32::min);
        if lowest.is_finite() {
            self.fader.hold_for(lowest, self.sample_rate);
        }
        for frame_idx in 0..frames {
            // Per-voice constant-power panning: in ensemble mode each unison
            // player is seated at its own azimuth, set at note-on. Other
            // voices sit at pan 0, both channels equal.
            let mut mono = 0.0f32; // for the (global) sympathetic tail
            let mut sl = 0.0f32;
            let mut sr_ = 0.0f32;
            for i in 0..poly {
                let s = self.voices[i].process(&self.patch);
                mono += s;
                let pan = self.voices[i].pan();
                if pan.abs() < 1e-4 {
                    sl += s;
                    sr_ += s;
                } else {
                    let ang = (pan.clamp(-1.0, 1.0) + 1.0) * 0.5 * std::f32::consts::FRAC_PI_2;
                    sl += s * ang.cos() * std::f32::consts::SQRT_2;
                    sr_ += s * ang.sin() * std::f32::consts::SQRT_2;
                }
            }
            mono *= norm;
            sl *= norm;
            sr_ *= norm;
            // persistent ring: the open strings, sympathetic to whatever is
            // sounding. A pizzicato excites them as a bow does, so the tail
            // is one tail and does not branch on the stroke. The tail is a
            // global resonance: driven by the mono sum, added centered.
            let tail = self.symp.process(mono) * self.symp_level;
            let (l, r) = self.fader.run(sl + tail, sr_ + tail);

            self.peak_l = self.peak_l.max(l.abs());
            self.peak_r = self.peak_r.max(r.abs());

            let base = frame_idx * channels;
            if channels >= 2 {
                output[base] = l;
                output[base + 1] = r;
            } else if channels == 1 {
                output[base] = 0.5 * (l + r);
            }
        }

        let vc = self.voices[..poly].iter().filter(|v| v.is_active()).count();
        self.measure_load(started, frames);
        self.publish_meter(frames, vc);
    }

    /// The block's cost against its duration, kept as the worst of the
    /// recent past: one slow block in a stream is heard, and a figure that
    /// reads only the last block hides it.
    fn measure_load(&mut self, started: std::time::Instant, frames: usize) {
        const FORGET: f32 = 0.94;
        let period = frames as f32 / self.sample_rate;
        let load = started.elapsed().as_secs_f32() / period.max(1e-9);
        self.load = load.max(self.load * FORGET);
    }

    fn publish_meter(&mut self, frames: usize, voice_count: usize) {
        self.meter_counter += frames;
        let update_interval = (self.sample_rate / 30.0) as usize;
        if self.meter_counter >= update_interval {
            self.meter_counter = 0;
            self.meter_shadow.peak_l = self.peak_l;
            self.meter_shadow.peak_r = self.peak_r;
            self.meter_shadow.cpu_percent = self.load * 100.0;
            self.meter_shadow.voice_count = voice_count;
            self.meter_shadow.active_notes.clone_from(&self.held_keys);
            if self.patch_dirty || self.meter_shadow.patch_snapshot.is_none() {
                self.meter_shadow.patch_snapshot = Some(self.patch.clone());
                self.patch_dirty = false;
            }
            let slot = self.meter_writer.edit();
            slot.clone_from(&self.meter_shadow);
            self.meter_writer.publish();
            self.peak_l = 0.0;
            self.peak_r = 0.0;
        }
    }

    fn process_commands(&mut self) {
        while let Ok(cmd) = self.command_rx.try_recv() {
            match cmd {
                ArchetCommand::NoteOn(note, vel) => {
                    if !self.held_keys.contains(&note) { self.held_keys.push(note); }
                    self.note_seq += 1;
                    let seq = self.note_seq;
                    // A plucked string carries one note: the finger that stops
                    // the next one ends whatever still rings on that string.
                    // And the hand that plucks rests on the others, so a note
                    // whose key is already up, still fading, is ended by the
                    // next attack too; a note still held rings on, which is
                    // how a player lets one ring.
                    if self.patch.pluck {
                        let inst = ArchetVoice::inst_for(note, &self.patch);
                        let (string, _, _) = ArchetVoice::string_for(inst, note);
                        let poly = self.pool();
                        for v in self.voices[..poly].iter_mut() {
                            if let Some((i, s)) = v.on_string() {
                                if v.is_active() && i == inst && (s == string || v.is_releasing()) {
                                    v.stop_string();
                                }
                            }
                        }
                    }
                    if self.patch.ensemble >= 1.5 {
                        self.fire_unison(note, vel, seq);
                    } else {
                        let idx = self.allocate_voice_idx(note);
                        self.voices[idx].set_unison(0.0, 0.0, 0);
                        self.voices[idx].note_on(note, vel, &self.patch);
                        self.voices[idx].on_seq = seq;
                    }
                }
                ArchetCommand::NoteOnLegato(note, vel) => {
                    // Retune the loudest still-sounding voice (the bow that is
                    // down, even if just released), so the slur continues. Every
                    // note still gets a NoteOff, so no voice is held forever. No
                    // sounding voice:
                    // a fresh bow stroke.
                    let poly = self.pool();
                    let best = (0..poly)
                        .filter(|&i| self.voices[i].is_active() && self.voices[i].level() > 1e-4)
                        .max_by(|&a, &b| self.voices[a].level().partial_cmp(&self.voices[b].level()).unwrap());
                    self.note_seq += 1;
                    if let Some(i) = best {
                        self.voices[i].note_on_legato(note, vel, &self.patch);
                        self.voices[i].on_seq = self.note_seq;
                    } else {
                        let idx = self.allocate_voice_idx(note);
                        self.voices[idx].note_on(note, vel, &self.patch);
                        self.voices[idx].on_seq = self.note_seq;
                    }
                }
                ArchetCommand::BowChange(note, vel) => {
                    // Detache: reverse the bow on the still-bowing voice (no lift -> no
                    // pluck). Same voice-selection as legato; fresh stroke if none down.
                    let poly = self.pool();
                    let best = (0..poly)
                        .filter(|&i| self.voices[i].is_active() && self.voices[i].level() > 1e-4)
                        .max_by(|&a, &b| self.voices[a].level().partial_cmp(&self.voices[b].level()).unwrap());
                    self.note_seq += 1;
                    if let Some(i) = best {
                        self.voices[i].bow_change(note, vel, &self.patch);
                        self.voices[i].on_seq = self.note_seq;
                    } else {
                        let idx = self.allocate_voice_idx(note);
                        self.voices[idx].note_on(note, vel, &self.patch);
                        self.voices[idx].on_seq = self.note_seq;
                    }
                }
                ArchetCommand::NoteOff(note) => {
                    self.held_keys.retain(|&x| x != note);
                    // Release the oldest held cluster of this pitch (one off pairs
                    // with one on). In ensemble mode a NoteOn spawns a unison
                    // cluster sharing one on_seq, so release every voice of that
                    // pitch at the oldest seq -- not just one (else the cluster's
                    // other players hang). For a single voice this is identical to
                    // releasing the oldest. Repeated overlapping notes still choke
                    // correctly: only the oldest seq goes, the fresh one stays.
                    let oldest_seq = self.voices.iter()
                        .filter(|v| v.note == Some(note))
                        .map(|v| v.on_seq)
                        .min();
                    if let Some(seq) = oldest_seq {
                        // section release asynchrony: bows lift at slightly
                        // different times (~0-30 ms), not all on one sample.
                        let rmax = (0.030 * self.sample_rate) as i32;
                        let ens = self.patch.ensemble >= 1.5;
                        let mut k = 0u32;
                        for i in 0..self.voices.len() {
                            if self.voices[i].note == Some(note) && self.voices[i].on_seq == seq {
                                let d = if ens && k > 0 {
                                    let h = (note as u32)
                                        .wrapping_mul(2654435761)
                                        .wrapping_add(k.wrapping_mul(2246822519))
                                        .wrapping_add(seq as u32);
                                    (h % rmax as u32) as i32
                                } else { 0 };
                                self.voices[i].schedule_release(d, &self.patch);
                                k += 1;
                            }
                        }
                    }
                }
                ArchetCommand::AllNotesOff => {
                    self.held_keys.clear();
                    for v in self.voices.iter_mut() {
                        v.note_off(&self.patch);
                    }
                }
                ArchetCommand::PitchBend(_) => {}
                ArchetCommand::SetPolyphony(p) => {
                    self.patch.polyphony = p.clamp(1, MAX_VOICES as u8);
                    self.patch_dirty = true;
                }
                ArchetCommand::SetOutputLevel(l) => {
                    self.patch.output_level = l.clamp(0.0, 2.0);
                    self.patch_dirty = true;
                }
                ArchetCommand::LoadPatch(p) => {
                    self.load_patch(*p);
                }
                ArchetCommand::SetParam { param, value } => {
                    param.apply(&mut self.patch, value);
                    self.patch_dirty = true;
                    // Only the body choice touches the sympathetic strings;
                    // every other field is a coefficient the sounding voices
                    // read next block, so the pool is left alone and a note
                    // being bowed is not interrupted.
                    if param.needs_symp_rebuild() {
                        let inst = self.patch.instrument.body_index();
                        if inst != self.symp_inst {
                            self.symp = SympStrings::new(self.sample_rate, inst);
                            self.symp_inst = inst;
                        }
                    }
                }
            }
        }
    }

    /// Install a patch without going through the command channel. Mirrors the
    /// `LoadPatch` command exactly (dirty flag so the meter republishes,
    /// sympathetic-string rebuild on instrument change, per-desk re-seed). Used
    /// by the LoadPatch arm and by the sequencer's session-state restore so a
    /// loaded `.phx` reflects the saved patch (instrument, sympathetics, seed).
    pub fn load_patch(&mut self, p: ArchetPatch) {
        self.patch = p;
        // A saved patch can carry a polyphony above the voice pool; clamp
        // so the editor's mirror and the engine agree.
        self.patch.polyphony = self.patch.polyphony.clamp(1, MAX_VOICES as u8);
        self.patch_dirty = true;
        // The sympathetic open strings follow the instrument, bowed or
        // plucked alike: a pizzicato on a violin excites the other three
        // strings exactly as a bow does.
        let inst = self.patch.instrument.body_index();
        if inst != self.symp_inst {
            self.symp = SympStrings::new(self.sample_rate, inst);
            self.symp_inst = inst;
        }
        // Per-desk decorrelation for string sections: re-seed all voices so
        // this engine instance is independent of others, only when the
        // offset changes.
        if self.patch.seed_offset != 0 && self.patch.seed_offset != self.applied_seed {
            let off = self.patch.seed_offset;
            for v in &mut self.voices { v.reseed(off); }
            self.applied_seed = off;
        }
    }

    /// A section's note-on (Ternstroem, JASA, on unison frequency scatter;
    /// Meyer on orchestral sections): one note becomes `count` physical
    /// players, each with
    /// - a static F0 offset drawn about Gaussian, the scatter that diffuses
    ///   each partial into a band;
    /// - independent bow-noise, micro-pitch and vibrato-rate seeds (keyed
    ///   per voice_idx) and a random vibrato phase per note;
    /// - its own stage azimuth.
    ///
    /// The cost is the count of voices, not of engines.
    fn fire_unison(&mut self, note: u8, vel: u8, seq: u64) {
        // `ensemble` is the section size in players. Physical voices are
        // bounded at PHYS_CAP, where the richness of independent sources
        // saturates (Ternstroem); a larger section is louder, not wider.
        // A plucked section needs fewer players to read as many: their
        // attacks are already spread in time, and each note must leave the
        // pool room for the chords a pizzicato part writes.
        let cap = if self.patch.pluck { PLUCK_CAP } else { PHYS_CAP };
        let size = self.patch.ensemble.max(2.0);
        let count: usize = (size.round() as usize).clamp(2, cap);
        // The measured inter-player F0 dispersion of a section is 20-30
        // cents (Cuesta and Chandna, unison analysis; Ternstroem): 22 cents
        // SD here, with the per-voice slow drift on top.
        let sd_cents = 22.0;
        let onset_max = (0.035 * self.sample_rate) as u32; // ~35 ms attack spread
        // A seat's intonation is the player's: drawn from the seat and the
        // desk's seed, the same at every note, as a sum of three uniforms
        // for a bell-shaped spread; and the section tunes to one A, so the
        // seats' offsets are centred on it.
        let mut g = [0.0f32; PHYS_CAP];
        for (k, gk) in g.iter_mut().enumerate().take(count) {
            let seat = mix32(
                (k as u32)
                    .wrapping_mul(40503)
                    .wrapping_add(self.patch.seed_offset.wrapping_mul(2654435761)),
            );
            let u = |sh: u32| ((seat >> sh) & 0x3ff) as f32 / 1023.0;
            *gk = (u(0) + u(10) + u(20)) / 3.0 * 2.0 - 1.0;
        }
        let mean = g[..count].iter().sum::<f32>() / count as f32;
        for (k, gk) in g.iter().enumerate().take(count) {
            let det = (gk - mean) * sd_cents * 1.7; // 1.7: map the triangular-ish range to ~SD
            // seat the players across the desk: -0.7 .. +0.7
            let pan = if count > 1 {
                ((k as f32 / (count - 1) as f32) * 2.0 - 1.0) * 0.7
            } else { 0.0 };
            // Onset asynchrony is the note's: player 0 lands on time, the
            // rest up to about 28 ms late, differently at each note.
            let event = (note as u32)
                .wrapping_mul(2654435761)
                .wrapping_add((k as u32).wrapping_mul(40503))
                .wrapping_add(seq as u32);
            let onset = if k == 0 { 0 } else { ((event >> 5) % onset_max) as usize };
            let idx = self.allocate_voice_idx(note);
            self.voices[idx].set_unison(det, pan, onset);
            self.voices[idx].note_on(note, vel, &self.patch);
            self.voices[idx].on_seq = seq;
        }
    }

    /// Physical voices one note lights on this patch.
    fn voices_per_note(&self) -> usize {
        if self.patch.ensemble >= 1.5 {
            let cap = if self.patch.pluck { PLUCK_CAP } else { PHYS_CAP };
            (self.patch.ensemble.max(2.0).round() as usize).clamp(2, cap)
        } else {
            1
        }
    }

    /// The fader's gain now, for measurement.
    pub fn fader_gain(&self) -> f32 {
        self.fader.gain
    }

    /// Samples the output runs behind the notes: the fader's lookahead.
    pub const LATENCY: usize = Fader::LOOKAHEAD;

    /// The voices in use: the polyphony counts notes, and each note lights
    /// its players, up to the pool.
    fn pool(&self) -> usize {
        (self.patch.polyphony as usize)
            .saturating_mul(self.voices_per_note())
            .clamp(1, MAX_VOICES)
    }

    /// A voice for a new note: a free one, else the oldest of those already
    /// lifting, else the oldest sounding. Never a fixed index: a full pool
    /// would pile every player of a new note onto one voice.
    fn allocate_voice_idx(&mut self, note: u8) -> usize {
        let pool = self.pool();
        if let Some(i) = self.voices[..pool].iter().position(|v| !v.is_active()) {
            return i;
        }
        let _ = note;
        let oldest = |releasing: bool| {
            self.voices[..pool]
                .iter()
                .enumerate()
                .filter(|(_, v)| v.is_releasing() == releasing)
                .min_by_key(|(_, v)| v.on_seq)
                .map(|(i, _)| i)
        };
        oldest(true).or_else(|| oldest(false)).unwrap_or(0)
    }
}

#[cfg(test)]
mod preset_sweep_tests {
    use super::*;

    /// Archet is the most numerically delicate engine: the modal friction solver
    /// divides by bow/string velocities and takes a discriminant square root, so
    /// a degenerate preset could emit NaN/Inf, and a non-finite sample silences
    /// the whole master and propagates downstream. Sweep every factory preset on
    /// a held note and assert every sample is finite; a bank-level floor also
    /// proves the bow/pluck excitation actually produces sound. A short render
    /// suffices: a blow-up shows up within the first excited blocks. Runs by
    /// default so a physical-model regression is caught in CI, not by ear.
    #[test]
    fn all_presets_render_finite() {
        let sr = 48_000.0f32;
        let bank = ArchetPatch::factory_presets();
        let block = 256usize;
        let held  = (0.09 * sr) as usize;
        let total = (0.12 * sr) as usize;
        let mut loudest = 0.0f32;
        for p in &bank {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let _ = tx.send(ArchetCommand::LoadPatch(Box::new(p.clone())));
            let _ = tx.send(ArchetCommand::NoteOn(62, 100)); // D4
            let mut buf = vec![0.0f32; block * 2];
            let mut released = false;
            let mut frame = 0usize;
            while frame < total {
                if !released && frame >= held {
                    let _ = tx.send(ArchetCommand::NoteOff(62));
                    released = true;
                }
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for &s in &buf {
                    assert!(s.is_finite(),
                        "archet preset '{}' produced a non-finite sample ({s})", p.name);
                    loudest = loudest.max(s.abs());
                }
                frame += block;
            }
        }
        assert!(loudest > 1e-3,
            "no archet preset produced audible output - excitation path is broken");
    }

    /// Byte-identity golden for the multi-voice render path: ensemble mode
    /// (many active voices, per-voice constant-power pan, the global
    /// sympathetic tail on the mono sum), so the voice-summation order
    /// and the stereo split are covered. Update GOLDEN only for a deliberate,
    /// ear-verified sound change, never to paper over a drift.
    #[test]
    fn ensemble_render_bit_identical() {
        let sr = 48_000.0f32;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = ArchetPatch::violin_ensemble();
        p.polyphony = 8;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        tx.send(ArchetCommand::NoteOn(55, 100)).unwrap();
        tx.send(ArchetCommand::NoteOn(62, 90)).unwrap();
        let block = 128usize; // the live control-rate sub-block
        let nblocks = (0.8 * sr) as usize / block;
        let mut left = Vec::with_capacity(nblocks * block);
        let mut buf = vec![0.0f32; block * 2];
        for b in 0..nblocks {
            if b == nblocks * 2 / 3 {
                tx.send(ArchetCommand::NoteOff(55)).unwrap();
                tx.send(ArchetCommand::NoteOff(62)).unwrap();
            }
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            left.extend(buf.iter().step_by(2));
        }
        let h = crate::fingerprint::of(&left);
        const GOLDEN: u64 = 0xe71b9722c3d36962; // the ensemble render, note-offs included
        assert_eq!(h, GOLDEN, "Archet ensemble render drifted from golden (hash {h:#018x})");
    }
}

#[cfg(test)]
mod section_pool {
    use super::*;

    /// A section's polyphony counts notes: four notes on a section of
    /// eight light thirty-two voices, and a fifth note takes the oldest
    /// note's voices, not one voice for all its players.
    #[test]
    fn a_section_chord_keeps_every_player() {
        let sr = 48_000.0_f32;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = crate::patch::ArchetPatch::violin();
        p.ensemble = 8.0;
        p.polyphony = 4;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        let mut buf = vec![0.0f32; 512];
        eng.process_audio(&mut buf, 2);
        for n in [60u8, 64, 67, 71] {
            tx.send(ArchetCommand::NoteOn(n, 90)).unwrap();
            eng.process_audio(&mut buf, 2);
        }
        let lit = |eng: &ArchetEngine, note: u8| eng.voices.iter().filter(|v| v.is_active() && v.note == Some(note)).count();
        assert_eq!(eng.voices.iter().filter(|v| v.is_active()).count(), 32);
        for n in [60u8, 64, 67, 71] {
            assert_eq!(lit(&eng, n), 8, "note {n}");
        }
        tx.send(ArchetCommand::NoteOn(74, 90)).unwrap();
        eng.process_audio(&mut buf, 2);
        assert_eq!(lit(&eng, 74), 8, "the fifth note lights all its players");
        assert_eq!(lit(&eng, 60), 0, "the oldest note gave way");
        assert_eq!(lit(&eng, 64), 8);
    }

    /// The seats keep their intonation from one note to the next, centred
    /// on the section's A.
    #[test]
    fn a_section_keeps_its_seats_from_note_to_note() {
        let sr = 48_000.0_f32;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = crate::patch::ArchetPatch::violin();
        p.ensemble = 8.0;
        p.polyphony = 4;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        let mut buf = vec![0.0f32; 512];
        eng.process_audio(&mut buf, 2);
        let seats = |eng: &ArchetEngine, note: u8| -> Vec<i32> {
            let mut d: Vec<i32> = eng
                .voices
                .iter()
                .filter(|v| v.is_active() && v.note == Some(note))
                .map(|v| (v.unison_det() * 10.0).round() as i32)
                .collect();
            d.sort_unstable();
            d
        };
        tx.send(ArchetCommand::NoteOn(60, 90)).unwrap();
        eng.process_audio(&mut buf, 2);
        let first = seats(&eng, 60);
        tx.send(ArchetCommand::NoteOn(67, 90)).unwrap();
        eng.process_audio(&mut buf, 2);
        let second = seats(&eng, 67);
        assert_eq!(first.len(), 8);
        assert_eq!(first, second, "the same eight seats at every note");
        let mean = first.iter().sum::<i32>() as f32 / 80.0;
        assert!(mean.abs() < 0.5, "centred on the section's A, mean {mean} cents");
        assert!(first.iter().any(|&d| d.abs() > 50), "spread by more than a few cents");
    }
}

#[cfg(test)]
mod profile {
    use super::*;

    /// How long a pizzicato actually rings, on each open violin string.
    ///
    /// One note, plucked and left alone, per string; prints the times to
    /// -20 and -40 dB and the T60 fitted between them, and writes the audio.
    /// `pluck_repeat` cannot answer this: its notes overlap by design, so its
    /// tail is never one string's own decay.
    ///   cargo test --release --lib engine::profile::pizz_decay -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn pizz_decay() {
        let sr = 48_000.0_f32;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Pizzicato")
            .expect("the bank no longer has Violin Pizzicato");
        // The four open violin strings, each plucked once and left to ring.
        // `pluck_repeat` cannot show a decay: its notes overlap by design.
        println!("  string   note     -20 dB    -40 dB    fitted T60");
        for (name, pitch) in [("G3", 55u8), ("D4", 62), ("A4", 69), ("E5", 76)] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = preset.clone();
            p.polyphony = 1;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(pitch, 100)).unwrap();
            let block = 512usize;
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..((3.0 * sr) as usize / block) {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block { out.push(buf[i * 2]); }
            }
            // envelope in 10 ms hops, referenced to the peak
            let hop = (0.01 * sr) as usize;
            let env: Vec<f32> = out
                .chunks(hop)
                .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
                .collect();
            let peak = env.iter().cloned().fold(0.0f32, f32::max).max(1e-12);
            let at = |target_db: f32| -> f32 {
                env.iter()
                    .position(|&e| 20.0 * (e / peak).log10() <= target_db)
                    .map(|i| i as f32 * 0.01)
                    .unwrap_or(f32::NAN)
            };
            let (t20, t40) = (at(-20.0), at(-40.0));
            let t60 = if t20.is_finite() && t40.is_finite() && t40 > t20 {
                60.0 * (t40 - t20) / 20.0
            } else {
                f32::NAN
            };
            println!("  {name:>6}   {pitch:>4}   {t20:7.2} s {t40:7.2} s   {t60:7.2} s");
            write_wav(&format!("/tmp/archet_pizz_{name}.wav"), &out, sr);
        }
        println!("wrote /tmp/archet_pizz_{{G3,D4,A4,E5}}.wav");
    }

    /// The lowest open string of each instrument, plucked and left to ring,
    /// long enough for a bass to show its decay.
    ///
    /// One note per instrument on its plucked patch, the viola borrowing the
    /// violin's with the instrument changed, six seconds each, written out so
    /// each partial's decay can be read against the measured table it comes
    /// from. The violin harness stops at three seconds and covers only the
    /// violin.
    ///   cargo test --lib engine::profile::pizz_family -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn pizz_family() {
        use crate::patch::{ArchetPatch, Instrument};
        let sr = 48_000.0_f32;
        let block = 512usize;
        let bank = ArchetPatch::factory_presets();
        let find = |n: &str| {
            bank.iter()
                .find(|p| p.name == n)
                .unwrap_or_else(|| panic!("the bank no longer has {n}"))
                .clone()
        };
        let mut viola = find("Violin Pizzicato");
        viola.instrument = Instrument::Viola;
        // every open string of every instrument, lowest first
        let cases = [
            ("violin", find("Violin Pizzicato"), [55u8, 62, 69, 76]),
            ("viola", viola, [48, 55, 62, 69]),
            ("cello", find("Cello Pizzicato"), [36, 43, 50, 57]),
            ("bass", find("Bass Pizzicato"), [28, 33, 38, 43]),
        ];
        for (name, preset, opens) in cases {
            for (s, &pitch) in opens.iter().enumerate() {
                let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
                let mut p = preset.clone();
                p.polyphony = 1;
                tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
                tx.send(ArchetCommand::NoteOn(pitch, 100)).unwrap();
                let mut out: Vec<f32> = Vec::new();
                let mut buf = vec![0.0f32; block * 2];
                for _ in 0..((6.0 * sr) as usize / block) {
                    buf.fill(0.0);
                    eng.process_audio(&mut buf, 2);
                    for i in 0..block {
                        out.push(buf[i * 2]);
                    }
                }
                let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                println!("  {name:<7} string {s} note {pitch:>3}  peak {peak:.4}");
                write_wav(&format!("/tmp/archet_pizz_family_{name}_{s}.wav"), &out, sr);
            }
        }
        println!("wrote /tmp/archet_pizz_family_{{violin,viola,cello,bass}}_{{0..3}}.wav");
    }

    /// Whether the pluck position really moves from one note to the next.
    ///
    /// One pitch plucked over and over, with a gap long enough that each note
    /// decays on its own, so every note draws its own position and the comb it
    /// leaves can be fitted note by note. `pizz_decay` cannot show this: one
    /// note has one position, and four notes are four samples.
    ///   cargo test --lib engine::profile::pizz_spread -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn pizz_spread() {
        let sr = 48_000.0_f32;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Pizzicato")
            .expect("the bank no longer has Violin Pizzicato");
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = preset.clone();
        p.polyphony = 1;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        // D4: the open string whose fifth harmonic a fixed position buried
        // deepest.
        let pitch = 62u8;
        let notes = 10usize;
        let block = 512usize;
        let per = (1.2 * sr) as usize / block;
        let mut out: Vec<f32> = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        println!("  pluck     peak");
        for k in 0..notes {
            tx.send(ArchetCommand::NoteOn(pitch, 100)).unwrap();
            let start = out.len();
            for _ in 0..per {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            let peak = out[start..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
            println!("  {:>5}   {peak:7.4}", k + 1);
            tx.send(ArchetCommand::NoteOff(pitch)).unwrap();
        }
        write_wav("/tmp/archet_pizz_spread.wav", &out, sr);
        println!("wrote /tmp/archet_pizz_spread.wav, one pitch plucked {notes} times");
    }

    /// Whether the upper register stays inside full scale.
    ///
    /// The pluck corner follows the pitch and has no ceiling, so a high note
    /// releases a shape reaching further up in rank than a low one's, and the
    /// bridge weights the modes by rank. An analytic peak does not settle this:
    /// the shape term falls towards the treble while the rendered peak rises,
    /// so the body and the integrator weigh as much as the excitation. One
    /// pluck every few semitones at full velocity, the loudest case there is.
    ///   cargo test --lib engine::profile::pizz_register -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn pizz_register() {
        let sr = 48_000.0_f32;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Pizzicato")
            .expect("the bank no longer has Violin Pizzicato");
        let block = 512usize;
        let per = (1.2 * sr) as usize / block;
        let mut all: Vec<f32> = Vec::new();
        println!("  pitch     peak   at full scale");
        for pitch in (55u8..=96).step_by(3) {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = preset.clone();
            p.polyphony = 1;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(pitch, 127)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..per {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let full = out.iter().filter(|x| x.abs() >= 0.999).count();
            println!("  {pitch:>5}   {peak:6.4}   {full:>13}");
            all.extend_from_slice(&out);
        }
        write_wav("/tmp/archet_pizz_register.wav", &all, sr);
        println!("wrote /tmp/archet_pizz_register.wav");
    }

    /// A pizzicato phrase, for judging by ear what no measurement settles.
    ///
    /// The analysis harnesses force a single voice, so none of them can play a
    /// line: every note would cut the one before it. A plucked note rings well
    /// past its written length, and that overlap is most of what makes a
    /// pizzicato passage sound played rather than tested, so this one keeps
    /// enough voices for each tail to stand. The line crosses all four strings
    /// and climbs to the top of the register, where the excitation corner moved
    /// furthest, and it repeats notes so the pluck position can be heard moving
    /// between them.
    ///   cargo test --lib engine::profile::pizz_melody -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn pizz_melody() {
        let sr = 48_000.0_f32;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Pizzicato")
            .expect("the bank no longer has Violin Pizzicato");
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = preset.clone();
        // Enough voices that a ringing note is never stolen by the next attack:
        // the longest tail here outlasts several notes of the line.
        p.polyphony = 8;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        let beat = 60.0 / 126.0;
        // pitch, velocity, written length in beats
        let phrase: &[(u8, u8, f32)] = &[
            (69, 100, 0.5), (71, 92, 0.5), (72, 96, 0.5), (74, 100, 0.5),
            (76, 110, 1.0), (76, 88, 0.5), (74, 96, 0.5), (72, 100, 1.0),
            (69, 104, 1.0),
            (62, 100, 0.5), (69, 96, 0.5), (65, 100, 0.5), (62, 96, 0.5),
            (57, 104, 1.0), (55, 108, 1.0), (62, 100, 0.5), (67, 96, 0.5),
            (81, 104, 0.5), (83, 100, 0.5), (84, 104, 0.5), (86, 100, 0.5),
            (88, 112, 1.0), (84, 96, 0.5), (81, 100, 0.5), (76, 108, 1.5),
            (55, 100, 0.5), (62, 100, 0.5), (69, 104, 0.5), (76, 108, 0.5),
            (81, 112, 2.0),
        ];
        let block = 512usize;
        let mut out: Vec<f32> = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        let written: f32 = phrase.iter().map(|n| n.2).sum::<f32>() * beat;
        let mut pending: Vec<(f32, u8)> = Vec::new();
        let mut next = 0usize;
        let mut onset = 0.0f32;
        let mut t = 0.0f32;
        while t < written + 2.5 {
            while next < phrase.len() && onset <= t {
                let (pitch, vel, len) = phrase[next];
                tx.send(ArchetCommand::NoteOn(pitch, vel)).unwrap();
                pending.push((onset + len * beat, pitch));
                onset += len * beat;
                next += 1;
            }
            pending.retain(|&(when, pitch)| {
                if when > t {
                    return true;
                }
                tx.send(ArchetCommand::NoteOff(pitch)).unwrap();
                false
            });
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            for i in 0..block {
                out.push(buf[i * 2]);
            }
            t += block as f32 / sr;
        }
        let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let full = out.iter().filter(|x| x.abs() >= 0.999).count();
        println!(
            "  {} notes, {:.1} s, peak {peak:.4}, {full} samples at full scale",
            phrase.len(),
            out.len() as f32 / sr
        );
        write_wav("/tmp/archet_pizz_melody.wav", &out, sr);
        println!("wrote /tmp/archet_pizz_melody.wav");
    }

    /// Whether the release control actually shortens a bowed note.
    ///
    /// One pitch bowed and then let go, at several values of the control,
    /// reporting how long the tail takes to fall from the level it held when
    /// the bow left. A moved audio pin only proves the path changed; this shows
    /// whether the control spans anything a player would hear. The modal
    /// release factor is gated on the gesture having fallen away first, so the
    /// tail can be governed by that ramp rather than by the control, and this
    /// is what would show it.
    ///   cargo test --lib engine::profile::bow_release -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn bow_release() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        println!("  release    -20 dB     -40 dB");
        for secs in [0.02f32, 0.08, 0.25, 0.60] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = ArchetPatch::violin();
            p.polyphony = 1;
            p.release = secs;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(69, 100)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..((0.8 * sr) as usize / block) {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            let off = out.len();
            tx.send(ArchetCommand::NoteOff(69)).unwrap();
            for _ in 0..((2.0 * sr) as usize / block) {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            let hop = (0.005 * sr) as usize;
            let env: Vec<f32> = out[off..]
                .chunks(hop)
                .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
                .collect();
            let r = env.first().copied().unwrap_or(0.0).max(1e-12);
            let at = |db: f32| -> f32 {
                env.iter()
                    .position(|&e| 20.0 * (e / r).log10() <= db)
                    .map(|i| i as f32 * 0.005)
                    .unwrap_or(f32::NAN)
            };
            println!("  {secs:7.2}  {:7.3} s  {:7.3} s", at(-20.0), at(-40.0));
            write_wav(&format!("/tmp/archet_release_{:.0}ms.wav", secs * 1000.0), &out, sr);
        }
        println!("wrote /tmp/archet_release_*.wav");
    }

    /// A bowed phrase at two release settings, to hear whether a long
    /// release blurs the line: notes overlapping because the previous one
    /// has not let go is the thing to listen for, and no measurement
    /// settles it.
    ///   cargo test --lib engine::profile::bow_phrase -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn bow_phrase() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        let beat = 60.0 / 108.0;
        // pitch, velocity, written length in beats
        let phrase: &[(u8, u8, f32)] = &[
            (69, 96, 0.5), (71, 92, 0.5), (72, 100, 1.0), (74, 96, 0.5),
            (76, 108, 1.5), (74, 92, 0.5), (72, 96, 0.5), (69, 100, 1.5),
            (62, 100, 0.5), (66, 96, 0.5), (69, 104, 1.0), (67, 96, 0.5),
            (64, 100, 0.5), (62, 104, 2.0),
        ];
        for secs in [0.06f32, 0.25] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = ArchetPatch::violin();
            p.polyphony = 8;
            p.release = secs;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            let written: f32 = phrase.iter().map(|n| n.2).sum::<f32>() * beat;
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            let mut pending: Vec<(f32, u8)> = Vec::new();
            let mut next = 0usize;
            let mut onset = 0.0f32;
            let mut t = 0.0f32;
            while t < written + 2.0 {
                while next < phrase.len() && onset <= t {
                    let (pitch, vel, len) = phrase[next];
                    tx.send(ArchetCommand::NoteOn(pitch, vel)).unwrap();
                    pending.push((onset + len * beat, pitch));
                    onset += len * beat;
                    next += 1;
                }
                pending.retain(|&(when, pitch)| {
                    if when > t {
                        return true;
                    }
                    tx.send(ArchetCommand::NoteOff(pitch)).unwrap();
                    false
                });
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
                t += block as f32 / sr;
            }
            let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            println!("  release {secs:.2} s: {:.1} s, peak {peak:.4}", out.len() as f32 / sr);
            write_wav(&format!("/tmp/archet_bowphrase_{:.0}ms.wav", secs * 1000.0), &out, sr);
        }
        println!("wrote /tmp/archet_bowphrase_*.wav");
    }

    /// The balance of the bank: one note, every factory preset.
    ///
    /// The voice-sum norm decides how a solo patch sits against a desk, and
    /// nothing else here measures that: the chord probe is solo, the register
    /// sweep is wired to one preset, and the ensemble pin reports no level. A
    /// desk note lights several voices that sum incoherently, so the norm has
    /// to divide them back down to where a solo note sits; whether it does is
    /// a measurement, not an argument.
    ///
    /// Peak and a settled RMS are both printed because the bank mixes
    /// articulations: a plucked preset decays through the window a bowed one
    /// sustains, so neither number alone compares them.
    ///   cargo test --lib engine::profile::preset_levels -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn preset_levels() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        println!("  {:<22} {:>5} {:>5} {:>8} {:>10}", "preset", "poly", "ens", "peak", "rms 0.3-1s");
        for preset in crate::patch::ArchetPatch::factory_presets() {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let (name, poly, ens) = (preset.name.clone(), preset.polyphony, preset.ensemble);
            tx.send(ArchetCommand::LoadPatch(Box::new(preset))).unwrap();
            tx.send(ArchetCommand::NoteOn(69, 100)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..((1.0 * sr) as usize / block) {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let a = (0.3 * sr) as usize;
            let w = &out[a.min(out.len())..];
            let rms = if w.is_empty() {
                0.0
            } else {
                (w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32).sqrt()
            };
            println!("  {name:<22} {poly:>5} {ens:>5.1} {peak:>8.4} {rms:>10.5}");
        }
    }

    /// The four instruments across their own registers at the same
    /// velocity: the mean rms of five notes spanning each range, and the
    /// level that would bring it to the violin's.
    ///   cargo test --lib engine::profile::inst_balance -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn inst_balance() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        let mk = [ArchetPatch::violin(), ArchetPatch::viola(), ArchetPatch::cello(), ArchetPatch::double_bass()];
        let registers: [[u8; 5]; 4] = [[55, 62, 69, 76, 83], [48, 55, 62, 69, 76], [36, 43, 50, 57, 64], [28, 35, 42, 49, 55]];
        let mut mean = [0.0f32; 4];
        println!("  {:<12} {:>5} {:>8} {:>8}", "instrument", "note", "peak dB", "rms dB");
        for (i, (p, reg)) in mk.iter().zip(registers).enumerate() {
            for note in reg {
                let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
                tx.send(ArchetCommand::LoadPatch(Box::new(p.clone()))).unwrap();
                tx.send(ArchetCommand::NoteOn(note, 100)).unwrap();
                let mut out: Vec<f32> = Vec::new();
                let mut buf = vec![0.0f32; block * 2];
                for _ in 0..((1.5 * sr) as usize / block) {
                    buf.fill(0.0);
                    eng.process_audio(&mut buf, 2);
                    out.extend(buf.iter().step_by(2));
                }
                let w = &out[(0.5 * sr) as usize..];
                let peak = w.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                let sq = w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32;
                mean[i] += sq / reg.len() as f32;
                println!("  {:<12} {:>5} {:>8.1} {:>8.1}", p.name, note,
                         20.0 * peak.max(1e-9).log10(), 10.0 * sq.max(1e-18).log10());
            }
        }
        println!("  {:<12} {:>9} {:>12}", "instrument", "mean dB", "level x");
        for (i, p) in mk.iter().enumerate() {
            println!("  {:<12} {:>9.1} {:>12.3}", p.name, 10.0 * mean[i].max(1e-18).log10(),
                     (mean[0] / mean[i]).sqrt() * crate::voice::ArchetVoice::inst_level(p.instrument.body_index()));
        }
    }

    /// Where a voice's time goes: the body bank and the modal string, each
    /// run alone for a second of samples, per instrument, with the number
    /// of resonators the body carries.
    ///   cargo test --release --lib engine::profile::voice_cost -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn voice_cost() {
        phonix_rt::denormal::enable_flush_to_zero();
        let sr = 48_000.0f32;
        let n = sr as usize;
        println!("  {:<6} {:>6} {:>10} {:>10}", "body", "modes", "body us/s", "string us/s");
        for (inst, f0) in [(0usize, 440.0f32), (1, 220.0), (2, 110.0), (3, 55.0)] {
            let mut body = crate::body::ModalBody::new(sr, inst, 1.0, 9.0);
            let modes = body.modes();
            let t = std::time::Instant::now();
            let mut acc = 0.0f32;
            for i in 0..n {
                acc += body.process(if i == 0 { 1.0 } else { 0.0 });
            }
            let body_us = t.elapsed().as_secs_f64() * 1e6;
            let mut string = crate::modal::ModalString::new(sr);
            string.set_voice(f0, 1.0, 0.02, 1e-4, 0.12);
            let t = std::time::Instant::now();
            for _ in 0..n {
                acc += string.process(0.3, 0.5);
            }
            let string_us = t.elapsed().as_secs_f64() * 1e6;
            println!("  {:<6} {:>6} {:>10.0} {:>10.0}   ({acc:.3})", inst, modes, body_us, string_us);
        }
    }

    /// Headroom on a dense chord, which is where a solo patch has no guard.
    ///
    /// The voice-sum norm divides by the voices one NOTE lights, so a solo
    /// patch is not divided at all and overlapping notes sum freely, as the
    /// strings of a real instrument do. Nothing else here measures that: the
    /// register sweep builds a fresh engine per pitch, so its notes can never
    /// overlap, and the phrases play well under full velocity. A chord held at
    /// full velocity, filling the voice pool, is the worst a player can ask for.
    ///   cargo test --lib engine::profile::chord_headroom -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn chord_headroom() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        println!("  voices     peak   at full scale   headroom");
        for n in [1usize, 2, 4, 6, 8] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let p = ArchetPatch::violin(); // solo: the norm divides by one
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            // A spread chord, every note at full velocity, filling the pool.
            for (k, pitch) in [55u8, 62, 67, 71, 74, 79, 83, 86].iter().take(n).enumerate() {
                let _ = k;
                tx.send(ArchetCommand::NoteOn(*pitch, 127)).unwrap();
            }
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..((1.5 * sr) as usize / block) {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            let full = out.iter().filter(|x| x.abs() >= 0.999).count();
            let head = 20.0 * (1.0 / peak.max(1e-12)).log10();
            println!("  {n:>6}   {peak:6.4}   {full:>13}   {head:+6.1} dB");
        }
    }

    /// How a written note is taken: plucked, a fresh or changed bow, or the
    /// continuation of a slur.
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum Art {
        Pizz,
        Arco,
        Slur,
    }

    /// A written note: desk, onset and length in beats, written pitch.
    #[derive(Clone, Copy, Debug)]
    struct Written {
        desk: usize,
        onset: f32,
        midi: u8,
        len: f32,
        art: Art,
    }

    /// A piece as written: its desks named by instrument, its notes, and its
    /// marks as (bar, desk the mark applies to or None for all, mark).
    struct Piece {
        bpm: f32,
        beats_per_bar: usize,
        desks: Vec<String>,
        notes: Vec<Written>,
        marks: Vec<(usize, Option<usize>, String)>,
    }

    /// A note as played: desk, onset in beats, sounding pitch, velocity,
    /// length in beats, and how it is taken.
    type Played = (usize, f32, u8, u8, f32, Art);

    /// The passage `score_render` plays with no file named: the opening of
    /// the third movement of Tchaikovsky's fourth symphony, Scherzo,
    /// pizzicato ostinato, bars one to sixteen, every desk. Read off the
    /// Jurgenson-lineage full score and the first-violin part on IMSLP and
    /// cross-checked against a Breitkopf reprint. Two-four, F major, all five
    /// desks from the first bar, "pizzicato sempre", piano with a swell
    /// across bars five to eight and again across thirteen to sixteen. The
    /// public-domain scores carry no metronome mark, so the tempo is the one
    /// editorial figure in circulation. The double bass is written an octave
    /// above where it sounds, and sounds here.
    fn score() -> Piece {
        // Four eighth slots per bar, a rest as an empty slot, a double stop
        // as two pitches. Bars nine to fifteen repeat one to seven; bar
        // sixteen is its own.
        const VLN1: [[&[u8]; 4]; 8] = [
            [&[65], &[64], &[62], &[60]],
            [&[62], &[64], &[65], &[69]],
            [&[72], &[], &[72], &[74]],
            [&[72], &[], &[72], &[74]],
            [&[72], &[74], &[72], &[74]],
            [&[72], &[74], &[72], &[77]],
            [&[76], &[74], &[72], &[70]],
            [&[69], &[67], &[65], &[64]],
        ];
        const VLN1_16: [&[u8]; 4] = [&[69], &[71], &[69, 76], &[]];
        const VLN2: [[&[u8]; 4]; 8] = [
            [&[60], &[60], &[57], &[57]],
            [&[57], &[60], &[60], &[65]],
            [&[], &[72], &[65, 69], &[65, 69]],
            [&[], &[72], &[65, 69], &[65, 69]],
            [&[65, 69], &[65, 69], &[65, 69], &[65, 69]],
            [&[65, 69], &[65, 69], &[65, 69], &[72]],
            [&[72], &[69], &[69], &[65]],
            [&[65], &[62], &[62], &[60]],
        ];
        const VLN2_16: [&[u8]; 4] = [&[64], &[65], &[64], &[]];
        const VLA: [[&[u8]; 4]; 8] = [
            [&[57], &[57], &[53], &[53]],
            [&[53], &[55], &[57], &[60]],
            [&[], &[], &[60], &[62]],
            [&[], &[], &[60], &[62]],
            [&[60], &[62], &[60], &[62]],
            [&[60], &[62], &[60], &[65]],
            [&[67], &[62], &[64], &[58]],
            [&[60], &[55], &[57], &[55]],
        ];
        const VLA_16: [&[u8]; 4] = [&[60], &[59], &[60], &[]];
        const VLC: [[&[u8]; 4]; 8] = [
            [&[41], &[45], &[50], &[53]],
            [&[50], &[48], &[45], &[41]],
            [&[], &[], &[53], &[50, 57]],
            [&[], &[], &[53], &[50, 57]],
            [&[53, 57], &[50, 57], &[53, 57], &[50, 57]],
            [&[53, 57], &[50, 57], &[53, 57], &[50, 57]],
            [&[60], &[53], &[57], &[50]],
            [&[53], &[46], &[50], &[48]],
        ];
        const VLC_16: [&[u8]; 4] = [&[57], &[50], &[57], &[45]];
        const CB: [[&[u8]; 4]; 8] = [
            [&[29], &[33], &[38], &[41]],
            [&[38], &[36], &[33], &[29]],
            [&[], &[], &[41], &[38]],
            [&[], &[], &[41], &[38]],
            [&[41], &[38], &[41], &[38]],
            [&[41], &[38], &[41], &[45]],
            [&[48], &[41], &[45], &[38]],
            [&[41], &[34], &[38], &[36]],
        ];
        const CB_16: [&[u8]; 4] = [&[45], &[38], &[45], &[33]];
        type Bars = [[&'static [u8]; 4]; 8];
        type Bar = [&'static [u8]; 4];
        let desks: [(&Bars, &Bar); 5] = [
            (&VLN1, &VLN1_16),
            (&VLN2, &VLN2_16),
            (&VLA, &VLA_16),
            (&VLC, &VLC_16),
            (&CB, &CB_16),
        ];
        // Written notes as (desk, onset in eighths, written pitch, length in
        // eighths), and the marks as (bar, mark). The bass is written an
        // octave above where it sounds, so its written pitch is kept here and
        // transposed with the rest. Bar fifteen closes on a B natural in the
        // first violins and violas where bar seven had the flat.
        let mut notes = Vec::new();
        for (desk, (bars, last)) in desks.iter().enumerate() {
            for bar in 1..=16usize {
                let slots: &[&[u8]; 4] = if bar == 16 { last } else { &bars[(bar - 1) % 8] };
                for (slot, ps) in slots.iter().enumerate() {
                    for &p in ps.iter() {
                        let natural = bar == 15 && slot == 3 && (desk == 0 || desk == 2);
                        let p = if natural { p + 1 } else { p };
                        let written = if desk == 4 { p + 12 } else { p };
                        notes.push(Written {
                            desk,
                            onset: (bar as f32 - 1.0) * 2.0 + slot as f32 * 0.5,
                            midi: written,
                            len: 0.5,
                            art: Art::Pizz,
                        });
                    }
                }
            }
        }
        let marks = [(1, "p"), (5, "<"), (7, ">"), (13, "<"), (15, ">")]
            .iter()
            .map(|&(b, m)| (b, None, m.to_string()))
            .collect();
        Piece {
            bpm: 172.0,
            beats_per_bar: 2,
            desks: ["violin", "violin", "viola", "cello", "bass"].iter().map(|s| s.to_string()).collect(),
            notes,
            marks,
        }
    }

    /// A piece read from files: the whole of a work rather than an excerpt.
    ///
    /// The directory named by ARCHET_SCORE holds three files, each with a
    /// header line. piece.csv gives key,value lines: bpm; beats_per_bar; and
    /// desks, the instrument of each desk in order (violin, viola, cello or
    /// bass), which chooses the presets and the octave the bass sounds at.
    /// notes.csv has one note per line as desk, onset in beats, written MIDI
    /// pitch, length in beats, articulation (pizz, arco, or arco-slur for a
    /// note continuing a slur), flag. marks.csv has one dynamic mark per
    /// line as bar, mark; a mark written desk:mark applies to that desk only.
    fn score_from(dir: &str) -> Piece {
        let read = |name: &str| -> Vec<Vec<String>> {
            let path = format!("{dir}/{name}");
            std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
                .lines()
                .skip(1)
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.split(',').map(|f| f.trim().to_string()).collect())
                .collect()
        };
        fn field<T: std::str::FromStr>(f: &[String], i: usize) -> T {
            f.get(i)
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| panic!("malformed line {f:?}"))
        }
        let piece = read("piece.csv");
        let value = |key: &str| -> Vec<String> {
            piece
                .iter()
                .find(|f| f[0] == key)
                .unwrap_or_else(|| panic!("piece.csv has no {key}"))[1..]
                .to_vec()
        };
        let bpm: f32 = value("bpm")[0].parse().expect("bpm");
        let beats_per_bar: usize = value("beats_per_bar")[0].parse().expect("beats_per_bar");
        let desks = value("desks");
        let mut uncertain = 0usize;
        let notes: Vec<Written> = read("notes.csv")
            .iter()
            .map(|f| {
                if f.get(5).map(|s| s.contains('?')).unwrap_or(false) {
                    uncertain += 1;
                }
                let art = match f.get(4).map(String::as_str) {
                    Some("pizz") => Art::Pizz,
                    Some("arco") => Art::Arco,
                    Some("arco-slur") => Art::Slur,
                    other => panic!("unknown articulation {other:?} in {f:?}"),
                };
                Written { desk: field(f, 0), onset: field(f, 1), midi: field(f, 2), len: field(f, 3), art }
            })
            .collect();
        let marks: Vec<(usize, Option<usize>, String)> = read("marks.csv")
            .iter()
            .map(|f| {
                let m = f.get(1).cloned().unwrap_or_default();
                match m.split_once(':') {
                    Some((d, m)) => (field(f, 0), Some(d.parse().expect("desk of a mark")), m.to_string()),
                    None => (field(f, 0), None, m),
                }
            })
            .collect();
        let bars = notes.iter().map(|n| (n.onset / beats_per_bar as f32) as usize + 1).max().unwrap_or(0);
        println!(
            "  piece from {dir}: {} desks, {} notes over {bars} bars, {} marks, {uncertain} pitches flagged uncertain",
            desks.len(),
            notes.len(),
            marks.len()
        );
        Piece { bpm, beats_per_bar, desks, notes, marks }
    }

    /// The performance, kept apart from the notes.
    ///
    /// The dynamic follows the marks: a level mark sets the bar's velocity, a
    /// hairpin or a written crescendo or diminuendo ramps bar by bar to the
    /// next mark, and an unmarked swell rises by the amount the opening's
    /// hairpins do; a desk with marks of its own follows those. The first
    /// beat of each bar leans, the other beats a little, and what falls off
    /// the beat sits back; the tune sits over its accompaniment and the bass
    /// under it. And no attack lands on the grid: within one player the
    /// onsets spread by some ten milliseconds, and between the players of an
    /// ensemble by a few tens (Rasch, Synchronization in performed ensemble
    /// music, Acustica 43, 1979), the desks lagging the leader by fixed
    /// amounts. A double stop is one gesture, so both its notes move
    /// together. Each note is held its written length: a plucked note rings
    /// until the next one is taken, and the hand mutes it as that one
    /// sounds. The draws are seeded, so the passage renders the same every
    /// time.
    fn perform(piece: &Piece) -> Vec<Played> {
        const VOICE: [i32; 5] = [6, 0, 0, 0, -2];
        const DESK_LAG_MS: [f32; 5] = [0.0, 9.0, 13.0, 16.0, 19.0];
        const PLAYER_SPREAD_S: f32 = 0.030;
        let bpb = piece.beats_per_bar as f32;
        let bars = piece.notes.iter().map(|n| (n.onset / bpb) as usize + 1).max().unwrap_or(0);
        let tutti: Vec<(usize, &str)> =
            piece.marks.iter().filter(|m| m.1.is_none()).map(|m| (m.0, m.2.as_str())).collect();
        let own = |desk: usize| -> Vec<(usize, &str)> {
            piece.marks.iter().filter(|m| m.1 == Some(desk)).map(|m| (m.0, m.2.as_str())).collect()
        };
        let curves: Vec<(Vec<i32>, Vec<bool>)> = (0..piece.desks.len())
            .map(|d| {
                let mine = own(d);
                dynamics(bars, if mine.is_empty() { &tutti } else { &mine })
            })
            .collect();
        for (d, (dynamic, _)) in curves.iter().enumerate() {
            if d == 0 || !own(d).is_empty() {
                println!(
                    "  desk {d} dynamic by bar: {}",
                    dynamic[1..=bars].iter().map(|v| v.to_string()).collect::<Vec<_>>().join(" ")
                );
            }
        }
        let beat = 60.0 / piece.bpm;
        let mut seed: u32 = 0x2545_f491;
        let mut draw = move || -> f32 {
            let mut g = 0.0f32;
            for _ in 0..3 {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                g += (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            }
            g / 3.0
        };
        let mut s = Vec::new();
        let mut gesture: Option<((usize, f32), f32)> = None;
        for n in &piece.notes {
            let bar = (n.onset / bpb) as usize + 1;
            let in_bar = n.onset - (bar as f32 - 1.0) * bpb;
            let lean = if in_bar == 0.0 {
                6
            } else if in_bar.fract() == 0.0 {
                2
            } else {
                -2
            };
            let late = match gesture {
                Some((key, l)) if key == (n.desk, n.onset) => l,
                _ => {
                    let l = (DESK_LAG_MS[n.desk] / 1000.0 + draw() * PLAYER_SPREAD_S) / beat;
                    gesture = Some(((n.desk, n.onset), l));
                    l
                }
            };
            let on = (n.onset + late).max(0.0);
            let (dynamic, accent) = &curves[n.desk];
            let acc = if accent[bar] { 16 } else { 0 };
            let vel = (dynamic[bar] + acc + lean + VOICE[n.desk]).clamp(1, 127) as u8;
            let sounding = if piece.desks[n.desk] == "bass" { n.midi - 12 } else { n.midi };
            s.push((n.desk, on, sounding, vel, n.len, n.art));
        }
        s
    }

    /// The velocity of each bar from a set of marks, with the bars that
    /// carry an accent. Bars are 1-based; index 0 is unused.
    fn dynamics(bars: usize, marks: &[(usize, &str)]) -> (Vec<i32>, Vec<bool>) {
        const SWELL: i32 = 40;
        // The marks span the velocity range evenly, ppp to fff.
        let level_of = |m: &str| -> Option<i32> {
            Some(match m {
                "ppp" => 16,
                "pp" => 32,
                "p" => 48,
                "mp" => 64,
                "mf" => 80,
                "f" => 96,
                "ff" => 112,
                "fff" => 127,
                _ => return None,
            })
        };
        let mut marks: Vec<(usize, &str)> = marks.to_vec();
        marks.sort_by_key(|m| m.0);
        // Level marks carry forward; a hairpin then bends the bars between
        // itself and the next mark.
        let mut dynamic = vec![48i32; bars + 2];
        let mut current = 48;
        for (bar, d) in dynamic.iter_mut().enumerate().take(bars + 1).skip(1) {
            for &(b, m) in &marks {
                if b == bar {
                    if let Some(l) = level_of(m) {
                        current = l;
                    }
                }
            }
            *d = current;
        }
        let level = dynamic.clone();
        let mut accent = vec![false; bars + 2];
        // A hairpin or a written crescendo or diminuendo runs from its bar to
        // the next mark of any kind and aims at the next written level in
        // its direction. With no such level, a rise swells by a fixed amount
        // and a fall returns to the bar's carried level over the length of
        // the rise it answers. An opening hairpin's bars reach the peak on
        // its last bar; a closing one's bars step down, still above the
        // level on its last bar, from wherever the bar before left off.
        let mut last_rise = 1usize;
        for (i, &(b, m)) in marks.iter().enumerate() {
            let rising = matches!(m, "<" | "cresc");
            let falling = matches!(m, ">" | "dim");
            if m == "sf" {
                if b <= bars {
                    accent[b] = true;
                }
                continue;
            }
            if !(rising || falling) || b > bars {
                continue;
            }
            let next_any = marks.get(i + 1).map(|n| n.0);
            let next_level = marks[i + 1..]
                .iter()
                .find_map(|&(bb, mm)| level_of(mm).map(|l| (bb, l)));
            let from = if rising { dynamic[b] } else { dynamic[b.max(2) - 1] };
            // A closing hairpin glyph answers the opening one and is no
            // longer than it; a written diminuendo runs on to the next mark.
            let glyph_end = |n: Option<usize>| n.map_or(b + last_rise, |n| n.min(b + last_rise));
            let (to, end) = match (rising, next_level) {
                (true, Some((bb, l))) if l > from => (l, next_any.unwrap_or(bb)),
                (true, _) => (from + SWELL, next_any.unwrap_or(b + 1)),
                (false, Some((bb, l))) if l < from && m == ">" => (l, glyph_end(Some(bb))),
                (false, Some((bb, l))) if l < from => (l, next_any.unwrap_or(bb)),
                (false, _) => (level[b], glyph_end(next_any)),
            };
            let end = end.max(b + 1).min(bars + 1);
            let span = if rising { end - b } else { end - b + 1 };
            if rising {
                last_rise = span;
            }
            for (k, bar) in (b..end).enumerate() {
                if bar <= bars {
                    let t = (k + 1) as f32 / span as f32;
                    dynamic[bar] = from + ((to - from) as f32 * t).round() as i32;
                }
            }
        }
        (dynamic, accent)
    }

    /// Whether a finger landing on a string ends the note still ringing on it.
    ///
    /// Two notes on the G string, then the same first note followed by one on
    /// the D string, and in each case the first note's third partial read
    /// before and after the second attack. That partial is chosen to sit
    /// between the second note's harmonics, tens of hertz from the nearest,
    /// so it can be read through a narrow window without the new note leaking
    /// into it. A fundamental read the same way could not tell: the new note's
    /// harmonics sit close enough to set a floor of a few tens of dB, which is
    /// where a first attempt at this measurement landed.
    ///   cargo test --lib engine::profile::pizz_stop -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn pizz_stop() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Pizzicato")
            .expect("the bank no longer has Violin Pizzicato");
        // A stopped note on the G string, read at its third partial. An open
        // string would not do: the engine keeps sympathetic open strings that
        // ring on at exactly those partials after the voice itself is ended,
        // and a probe on that grid reads them, not the voice. This partial
        // sits tens of hertz from every open-string harmonic and from every
        // harmonic of both second notes.
        let first = 58u8; // Bb3, stopped on the G string
        let partial = 3.0 * 440.0 * 2f32.powf((first as f32 - 69.0) / 12.0);
        println!("  second note   string   before  +150 ms   +300 ms");
        for (second, which) in [(60u8, "same (G)"), (67u8, "other (D)")] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = preset.clone();
            p.polyphony = 8;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(first, 100)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            let gap = (0.4 * sr) as usize / block;
            for _ in 0..gap {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            let at = out.len();
            tx.send(ArchetCommand::NoteOn(second, 100)).unwrap();
            // One block in, say what the engine holds: which voices ring, on
            // which string, and whether the new attack ended any of them.
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            for i in 0..block {
                out.push(buf[i * 2]);
            }
            for (i, v) in eng.voices.iter().enumerate().take(8) {
                if v.is_active() {
                    println!(
                        "    voice {i}: note {:?}, on {:?}, stopped {}",
                        v.note,
                        v.on_string(),
                        v.stopped()
                    );
                }
            }
            for _ in 1..((0.6 * sr) as usize / block) {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    out.push(buf[i * 2]);
                }
            }
            // A Hann-windowed heterodyne on the first note's third partial. A
            // rectangular window leaks the fresh second note through its
            // sidelobes and swamps the old partial; Hann puts a harmonic tens
            // of hertz away far below anything the old partial can fall to.
            let win = (0.10 * sr) as usize;
            let level = |centre: usize| -> f32 {
                let lo = centre.saturating_sub(win / 2);
                let hi = (lo + win).min(out.len());
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (k, &s) in out[lo..hi].iter().enumerate() {
                    let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * k as f64 / win as f64).cos();
                    let ph = 2.0 * std::f64::consts::PI * partial as f64 * (lo + k) as f64 / sr as f64;
                    re += w * s as f64 * ph.cos();
                    im -= w * s as f64 * ph.sin();
                }
                (re * re + im * im).sqrt() as f32 / (hi - lo) as f32
            };
            let before = level(at - (0.06 * sr) as usize).max(1e-12);
            let db = |v: f32| 20.0 * (v.max(1e-12) / before).log10();
            println!(
                "  {second:>11}   {which:<8} {:>6.1}  {:>+7.1}  {:>+8.1}",
                0.0,
                db(level(at + (0.15 * sr) as usize)),
                db(level(at + (0.30 * sr) as usize))
            );
        }
        println!("  (dB against the level before the second attack; columns at +150 and +300 ms)");
    }

    /// Every parameter at its two ends, and whether the sound moved: level,
    /// centroid and fingerprint of one second of one note, on the solo
    /// violin bowed and plucked. A parameter that moves nothing on either
    /// is inert.
    ///   cargo test --release --lib engine::profile::param_audit -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn param_audit() {
        use crate::patch::ArchetParam as P;
        let sr = 48_000.0_f32;
        let block = 256usize;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let find = |n: &str| bank.iter().find(|p| p.name == n).unwrap_or_else(|| panic!("no {n}")).clone();
        let params: [(P, &str, f32, f32); 17] = [
            (P::BowPos, "bow_pos", 0.03, 0.20), (P::BowVel, "bow_vel", 0.02, 0.5),
            (P::BowForce, "bow_force", 0.2, 2.0), (P::BowNoise, "bow_noise", 0.0, 0.4),
            (P::Loss, "loss", 0.0, 0.6), (P::BridgeHillDb, "bridge_hill_db", 0.0, 15.0),
            (P::Attack, "attack", 0.005, 0.2),
            (P::Release, "release", 0.02, 0.4), (P::VelSens, "vel_sens", 0.0, 1.0),
            (P::VibRate, "vib_rate", 3.0, 8.0), (P::VibDepth, "vib_depth", 0.0, 30.0),
            (P::VibDelay, "vib_delay", 0.0, 0.8), (P::Ensemble, "ensemble", 0.0, 12.0),
            (P::TuneCents, "tune_cents", -50.0, 50.0), (P::Instrument, "instrument", 0.0, 3.0),
            (P::AutoRange, "auto_range", 0.0, 1.0), (P::Pluck, "pluck", 0.0, 1.0),
        ];
        let render = |preset: &crate::patch::ArchetPatch, param: P, value: f32| -> (f32, f32, u64) {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = preset.clone();
            param.apply(&mut p, value);
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            let mut buf = vec![0.0f32; block * 2];
            eng.process_audio(&mut buf, 2);
            tx.send(ArchetCommand::NoteOn(69, 100)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            let hold = (0.7 * sr) as usize / block;
            for i in 0..((1.0 * sr) as usize / block) {
                if i == hold {
                    tx.send(ArchetCommand::NoteOff(69)).unwrap();
                }
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for k in 0..block {
                    out.push(buf[k * 2]);
                }
            }
            let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len() as f32).sqrt();
            let level = 20.0 * rms.max(1e-9).log10();
            // centroid over the held part, by a coarse FFT-free estimate:
            // the zero-crossing rate scaled by the sample rate.
            let held = &out[(0.2 * sr) as usize..(0.7 * sr) as usize];
            let zc = held.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32;
            let centroid = zc / (2.0 * held.len() as f32) * sr;
            (level, centroid, crate::fingerprint::of(&out))
        };
        for name in ["Violin Solo", "Violin Pizzicato"] {
            let preset = find(name);
            println!("== {name}: parameter, level at min / max (dBFS), zero-crossing pitch at min / max (Hz)");
            for (param, label, lo, hi) in params.iter() {
                let a = render(&preset, *param, *lo);
                let b = render(&preset, *param, *hi);
                let inert = a.2 == b.2;
                println!(
                    "  {label:<15} {:>6.1} / {:>6.1}     {:>6.0} / {:>6.0}  {}",
                    a.0, b.0, a.1, b.1, if inert { "INERT" } else { "" }
                );
            }
        }
    }

    /// What the attack control does to a bowed onset: the rise of the
    /// envelope from a tenth to nine tenths of its level, at the control's
    /// two ends and its default.
    ///   cargo test --lib engine::profile::bow_attack -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn bow_attack() {
        let sr = 48_000.0_f32;
        let block = 64usize;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Solo")
            .expect("the bank no longer has Violin Solo");
        println!("  velocity  attack   settled within 1 dB by   rise 10-90 %   level at 20 / 50 / 100 / 200 ms (dB re 1 s)");
        for (vel, attack) in [60u8, 100, 127].iter().flat_map(|&v| [0.005f32, 0.01, 0.02, 0.04, 0.08, 0.15, 0.3].map(|a| (v, a))) {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = preset.clone();
            p.attack = attack;
            p.vib_depth = 0.0;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            let mut buf = vec![0.0f32; block * 2];
            eng.process_audio(&mut buf, 2);
            tx.send(ArchetCommand::NoteOn(69, vel)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            for _ in 0..((1.2 * sr) as usize / block) {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for k in 0..block {
                    out.push(buf[k * 2]);
                }
            }
            let hop = (0.002 * sr) as usize;
            let env: Vec<f32> = out
                .chunks(hop)
                .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
                .collect();
            let steady = env[(0.9 * sr) as usize / hop..].iter().sum::<f32>() / (env.len() - (0.9 * sr) as usize / hop) as f32;
            let at = |frac: f32| env.iter().position(|&e| e >= frac * steady).unwrap_or(env.len()) as f32 * hop as f32 / sr;
            let db = |t: f32| 20.0 * (env[(t * sr) as usize / hop] / steady).max(1e-6).log10();
            let settled = env
                .iter()
                .enumerate()
                .find(|(i, _)| env[*i..].iter().all(|&e| (e / steady).max(1e-6).log10().abs() * 20.0 <= 1.0))
                .map_or(f32::NAN, |(i, _)| i as f32 * hop as f32 / sr);
            println!(
                "  {vel:>5}  {attack:>6.3} s   {:>6.0} ms            {:>6.0} ms       {:>5.1} / {:>5.1} / {:>5.1} / {:>5.1}",
                settled * 1000.0,
                (at(0.9) - at(0.1)) * 1000.0,
                db(0.02),
                db(0.05),
                db(0.10),
                db(0.20)
            );
        }
    }

    /// The same note three times on the section: each note's pitch band,
    /// centre and width in cents, which a section of fixed players gives
    /// the same each time.
    ///   cargo test --release --lib engine::profile::section_repeat -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn section_repeat() {
        let sr = 48_000.0_f32;
        let block = 256usize;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Section")
            .expect("the bank no longer has Violin Section");
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = preset.clone();
        p.vib_depth = 0.0;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        let mut buf = vec![0.0f32; block * 2];
        eng.process_audio(&mut buf, 2);
        let mut out: Vec<f32> = Vec::new();
        let note_len = (1.0 * sr) as usize / block;
        let gap = (0.4 * sr) as usize / block;
        for _ in 0..3 {
            tx.send(ArchetCommand::NoteOn(69, 90)).unwrap();
            for _ in 0..note_len {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                out.extend(buf.iter().step_by(2));
            }
            tx.send(ArchetCommand::NoteOff(69)).unwrap();
            for _ in 0..gap {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                out.extend(buf.iter().step_by(2));
            }
        }
        println!("  note   band centre (cents re 440)   width at -6 dB (cents)");
        let period = (note_len + gap) * block;
        for n in 0..3 {
            let seg = &out[n * period + (0.4 * sr) as usize..n * period + (1.0 * sr) as usize];
            let len = seg.len();
            let mut mags = Vec::new();
            for cents in (-120..=120).step_by(4) {
                let f = 440.0 * 2f32.powf(cents as f32 / 1200.0);
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (k, &x) in seg.iter().enumerate() {
                    let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / len as f64).cos();
                    let ph = std::f64::consts::TAU * f as f64 * k as f64 / sr as f64;
                    re += w * x as f64 * ph.cos();
                    im -= w * x as f64 * ph.sin();
                }
                mags.push((cents as f64, re * re + im * im));
            }
            let total: f64 = mags.iter().map(|m| m.1).sum();
            let centre = mags.iter().map(|m| m.0 * m.1).sum::<f64>() / total.max(1e-30);
            let peak = mags.iter().map(|m| m.1).fold(0.0, f64::max);
            let above: Vec<f64> = mags.iter().filter(|m| m.1 >= peak * 0.25).map(|m| m.0).collect();
            let width = above.iter().cloned().fold(f64::MIN, f64::max) - above.iter().cloned().fold(f64::MAX, f64::min);
            println!("  {:>4}   {centre:>24.1}   {width:>22.0}", n + 1);
        }
    }

    /// A section of one, four, eight, sixteen and thirty players holding
    /// one note: level, the width of the fundamental's band, and the slow
    /// level modulation, with a file per size to hear.
    ///   cargo test --release --lib engine::profile::section_ladder -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn section_ladder() {
        let sr = 48_000.0_f32;
        let block = 256usize;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Section")
            .expect("the bank no longer has Violin Section");
        println!("  players   rms dBFS   fundamental band at -6 dB (cents)   level modulation < 2 Hz (dB std)");
        for players in [1.0f32, 4.0, 8.0, 16.0, 30.0] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = preset.clone();
            p.ensemble = players;
            p.polyphony = 32;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            let mut buf = vec![0.0f32; block * 2];
            eng.process_audio(&mut buf, 2);
            tx.send(ArchetCommand::NoteOn(69, 90)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            let hold = (3.0 * sr) as usize / block;
            for i in 0..hold + (1.0 * sr) as usize / block {
                if i == hold {
                    tx.send(ArchetCommand::NoteOff(69)).unwrap();
                }
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                out.extend_from_slice(&buf);
            }
            let mono: Vec<f32> = out.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
            let steady = &mono[(1.0 * sr) as usize..(3.0 * sr) as usize];
            let rms = (steady.iter().map(|x| x * x).sum::<f32>() / steady.len() as f32).sqrt();
            // the fundamental's band: a DFT over the steady part, 2 Hz per bin
            let n = steady.len();
            let f0 = 440.0f32;
            let mut mags = Vec::new();
            for cents in (-120..=120).step_by(4) {
                let f = f0 * 2f32.powf(cents as f32 / 1200.0);
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (k, &x) in steady.iter().enumerate() {
                    let w = 0.5 - 0.5 * (std::f64::consts::TAU * k as f64 / n as f64).cos();
                    let ph = std::f64::consts::TAU * f as f64 * k as f64 / sr as f64;
                    re += w * x as f64 * ph.cos();
                    im -= w * x as f64 * ph.sin();
                }
                mags.push((cents, (re * re + im * im).sqrt()));
            }
            let peak = mags.iter().map(|m| m.1).fold(0.0, f64::max);
            let above: Vec<i32> = mags.iter().filter(|m| m.1 >= peak * 0.5).map(|m| m.0).collect();
            let width = above.iter().max().unwrap_or(&0) - above.iter().min().unwrap_or(&0);
            // slow level modulation: 50 ms rms frames, their spread in dB
            let hop = (0.05 * sr) as usize;
            let frames: Vec<f32> = steady
                .chunks(hop)
                .map(|c| 20.0 * (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt().max(1e-9).log10())
                .collect();
            let mean = frames.iter().sum::<f32>() / frames.len() as f32;
            let std = (frames.iter().map(|f| (f - mean).powi(2)).sum::<f32>() / frames.len() as f32).sqrt();
            println!(
                "  {players:>7.0}   {:>8.1}   {width:>32}   {std:>30.2}",
                20.0 * rms.max(1e-9).log10()
            );
            let path = format!("/tmp/archet_section_{}.wav", players as u32);
            write_wav(&path, &mono, sr);
        }
        println!("  wrote /tmp/archet_section_{{1,4,8,16,30}}.wav");
    }

    /// The cost of the largest section: a four-note chord held on the
    /// largest violin section, timed against the audio it renders.
    ///   cargo test --release --lib engine::profile::section_load -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn section_load() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Section Large")
            .expect("the bank no longer has Violin Section Large");
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        tx.send(ArchetCommand::LoadPatch(Box::new(preset.clone()))).unwrap();
        for n in [60u8, 64, 67, 72] {
            tx.send(ArchetCommand::NoteOn(n, 110)).unwrap();
        }
        let secs = 5.0f32;
        let blocks = (secs * sr) as usize / block;
        let mut buf = vec![0.0f32; block * 2];
        let t0 = std::time::Instant::now();
        for _ in 0..blocks {
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
        }
        let wall = t0.elapsed().as_secs_f32();
        let lit = eng.voices.iter().filter(|v| v.is_active()).count();
        println!(
            "  {lit} voices lit: {secs:.1} s of audio in {wall:.2} s, {:.0} % of one core",
            100.0 * wall / secs
        );
        // The chord's peak over its last second, what a chain's ceiling
        // would have to hold.
        let mut peak = 0.0f32;
        for _ in 0..((1.0 * sr) as usize / block) {
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            peak = peak.max(buf.iter().fold(0.0f32, |m, x| m.max(x.abs())));
        }
        println!("  four-note chord peak in the bare engine: {peak:.2}");
    }

    /// A held bowed note at three dynamics, for comparison with a recording.
    ///
    /// The solo violin patch holds a B flat on the A string, a stopped note
    /// so the vibrato is in play, for eight seconds, then the bow lifts.
    /// Written to /tmp/archet_bow_held_<velocity>.wav.
    ///   cargo test --lib engine::profile::bow_held -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn bow_held() {
        let sr = 48_000.0_f32;
        let block = 512usize;
        let bank = crate::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Violin Solo")
            .expect("the bank no longer has Violin Solo");
        let note = 70u8;
        // Renders at the middle velocity with the vibrato off, for the
        // recordings that are played without one, at the preset's bow force
        // and at multiples of it.
        let mut runs = vec![(32u8, true, 1.0f32), (80, true, 1.0), (112, true, 1.0)];
        {
            let scale = 1.0f32;
            runs.push((80, false, scale));
        }
        for (vel, vib, force_scale) in runs {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = preset.clone();
            if !vib {
                p.vib_depth = 0.0;
                p.bow_force *= force_scale;
            }
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            let mut buf = vec![0.0f32; block * 2];
            eng.process_audio(&mut buf, 2);
            tx.send(ArchetCommand::NoteOn(note, vel)).unwrap();
            let mut out: Vec<f32> = Vec::new();
            let hold = (8.0 * sr) as usize / block;
            let tail = (1.5 * sr) as usize / block;
            let quarter = (0.25 * sr) as usize / block;
            let mut states = Vec::new();
            for i in 0..hold + tail {
                if i == hold {
                    tx.send(ArchetCommand::NoteOff(note)).unwrap();
                }
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for k in 0..block {
                    out.push(buf[k * 2]);
                }
                if (i + 1) % quarter == 0 && i < hold {
                    if let Some(v) = eng.voices.iter_mut().find(|v| v.is_active()) {
                        let (slip, force, speed) = v.bow_state();
                        states.push(format!("{:.0}%/{force:.2}/{speed:.2}", slip * 100.0));
                    }
                }
            }
            if !vib {
                println!("  force x{force_scale}: slipping share / bow force / bow speed per quarter second:");
                println!("  {}", states.join(" "));
            }
            let path = format!("/tmp/archet_bow_held_{vel}{}.wav", if vib { String::new() } else { format!("_novib_x{force_scale}") });
            write_wav(&path, &out, sr);
            let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            println!("  velocity {vel}: peak {peak:.4}, wrote {path}");
        }
    }

    /// A scored piece, desk by desk, on the solo patches.
    ///
    /// Each desk gets an engine on its instrument's bowed patch and one on
    /// its plucked patch, a single voice per note, and every desk is summed.
    /// The section patch cannot play a tutti: each of its notes lights eight
    /// voices on a pool of twenty-four, so three notes in it starts stealing.
    /// One player per desk keeps every note clean, which is what is being
    /// judged, and the desks sit at the balance the engine already gives each
    /// instrument. The viola has no plucked preset of its own, so it borrows
    /// the violin's with only the instrument changed. A bowed note is a fresh
    /// stroke after a rest, a bow change when it follows another note on the
    /// desk without one, and a legato when it continues a slur. ARCHET_SCORE
    /// names a directory holding a whole piece; unset, the opening bars
    /// written here are played.
    ///   cargo test --lib engine::profile::score_render -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn score_render() {
        use crate::patch::{ArchetPatch, Instrument};
        let sr = 48_000.0_f32;
        let block = 512usize;
        let bank = ArchetPatch::factory_presets();
        let find = |n: &str| {
            bank.iter()
                .find(|p| p.name == n)
                .unwrap_or_else(|| panic!("the bank no longer has {n}"))
                .clone()
        };
        let patches = |inst: &str| -> (ArchetPatch, ArchetPatch) {
            match inst {
                "violin" => (find("Violin Solo"), find("Violin Pizzicato")),
                "viola" => {
                    let mut pizz = find("Violin Pizzicato");
                    pizz.instrument = Instrument::Viola;
                    pizz.name = "Viola Pizzicato".into();
                    (find("Viola Solo"), pizz)
                }
                "cello" => (find("Cello Solo"), find("Cello Pizzicato")),
                "bass" => (find("Double Bass Solo"), find("Bass Pizzicato")),
                other => panic!("no presets for a desk of {other}"),
            }
        };
        let piece = match std::env::var("ARCHET_SCORE") {
            Ok(dir) => score_from(&dir),
            Err(_) => score(),
        };
        let mut score = perform(&piece);
        score.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let beat = 60.0 / piece.bpm;
        // One bowed and one plucked engine per desk, indexed as 2*desk and
        // 2*desk+1.
        let mut engines = Vec::new();
        for inst in &piece.desks {
            let (arco, pizz) = patches(inst);
            for p in [arco, pizz] {
                let (mut eng, tx, mr) = ArchetEngine::new_for_plugin(sr);
                tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
                let mut buf = vec![0.0f32; block * 2];
                eng.process_audio(&mut buf, 2);
                engines.push((eng, tx, mr));
            }
        }
        let end = score.iter().map(|n| n.1 + n.4).fold(0.0f32, f32::max) * beat + 3.0;
        let mut out: Vec<f32> = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        let mut mix = vec![0.0f32; block];
        let mut stems: Vec<Vec<f32>> = vec![Vec::new(); piece.desks.len()];
        let mut fired = vec![false; score.len()];
        let mut pending: Vec<(f32, usize, u8)> = Vec::new();
        // When the last bowed note of each desk ends, in beats, so the next
        // knows whether the bow stayed down.
        let mut bow_up_at = vec![f32::NEG_INFINITY; piece.desks.len()];
        let mut t = 0.0f32;
        let mut strokes = [0usize; 3];
        while t < end {
            // Releases due now go before the attacks due now: a key that
            // comes up at the same instant another goes down comes up first.
            pending.retain(|&(when, engine, pitch)| {
                if when > t {
                    return true;
                }
                engines[engine].1.send(ArchetCommand::NoteOff(pitch)).unwrap();
                false
            });
            for (i, &(desk, on, pitch, vel, len, art)) in score.iter().enumerate() {
                if fired[i] || on * beat > t {
                    continue;
                }
                let engine = 2 * desk + usize::from(art == Art::Pizz);
                let cmd = match art {
                    Art::Pizz => ArchetCommand::NoteOn(pitch, vel),
                    Art::Slur => {
                        strokes[2] += 1;
                        ArchetCommand::NoteOnLegato(pitch, vel)
                    }
                    Art::Arco if (on - bow_up_at[desk]).abs() * beat < 0.02 => {
                        strokes[1] += 1;
                        ArchetCommand::BowChange(pitch, vel)
                    }
                    Art::Arco => {
                        strokes[0] += 1;
                        ArchetCommand::NoteOn(pitch, vel)
                    }
                };
                if art != Art::Pizz {
                    bow_up_at[desk] = on + len;
                }
                engines[engine].1.send(cmd).unwrap();
                pending.push(((on + len) * beat, engine, pitch));
                fired[i] = true;
            }
            mix.iter_mut().for_each(|m| *m = 0.0);
            for (e, (eng, _, _)) in engines.iter_mut().enumerate() {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block {
                    mix[i] += buf[i * 2];
                    stems[e / 2].push(buf[i * 2]);
                }
            }
            out.extend_from_slice(&mix);
            t += block as f32 / sr;
        }
        let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let full = out.iter().filter(|x| x.abs() >= 0.999).count();
        println!(
            "  {} notes on {} desks, {:.1} s, peak {peak:.4}, {full} samples at full scale; bowed: {} fresh strokes, {} bow changes, {} slurred",
            score.len(),
            piece.desks.len(),
            out.len() as f32 / sr,
            strokes[0],
            strokes[1],
            strokes[2]
        );
        write_wav("/tmp/archet_score.wav", &out, sr);
        // Each desk on its own as well, so a fault heard in the sum can be
        // laid at one instrument's door.
        for (d, stem) in stems.iter().enumerate() {
            write_wav(&format!("/tmp/archet_score_desk{d}.wav"), stem, sr);
        }
        println!("wrote /tmp/archet_score.wav and /tmp/archet_score_desk{{0..{}}}.wav", piece.desks.len() - 1);
    }

    /// Acoustic-fit harness: render the violin patch at G3/D4/A4/D5/A5 ->
    /// /tmp/archet_<pitch>.wav, so the harmonic envelope can be measured and
    /// the body/string tuning driven OBJECTIVELY (no listening).
    ///   cargo test --lib engine::profile::violin_fit -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn violin_fit() {
        let sr = 48_000.0_f32;
        for pitch in [55u8, 62, 69, 74, 81] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = ArchetPatch::violin();
            p.polyphony = 1;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(pitch, 100)).unwrap();
            let block = 512usize;
            let n = (2.2 * sr) as usize / block;
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..n {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block { out.push(buf[i * 2]); }
            }
            write_wav(&format!("/tmp/archet_{}.wav", pitch), &out, sr);
        }
        println!("wrote /tmp/archet_{{55,62,69,74,81}}.wav");
    }

    /// Expression test: play D4 at rising velocities (a dynamic phrase) so a
    /// Python script can confirm loudness AND brightness vary note-to-note (the
    /// objective signature of "not mechanical"). -> /tmp/archet_phrase.wav
    ///   cargo test --lib engine::profile::phrase_dyn -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn phrase_dyn() {
        let sr = 48_000.0_f32;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = ArchetPatch::violin();
        p.polyphony = 2;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        let block = 512usize;
        let mut out: Vec<f32> = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        let mut play = |eng: &mut ArchetEngine, out: &mut Vec<f32>, vel: u8, secs: f32| {
            tx.send(ArchetCommand::NoteOn(62, vel)).unwrap();
            let nh = (secs * 0.7 * sr) as usize / block;
            let nt = (secs * 0.3 * sr) as usize / block;
            for _ in 0..nh { buf.fill(0.0); eng.process_audio(&mut buf, 2); for i in 0..block { out.push(buf[i*2]); } }
            tx.send(ArchetCommand::NoteOff(62)).unwrap();
            for _ in 0..nt { buf.fill(0.0); eng.process_audio(&mut buf, 2); for i in 0..block { out.push(buf[i*2]); } }
        };
        for vel in [35u8, 60, 85, 110] { play(&mut eng, &mut out, vel, 0.7); }
        write_wav("/tmp/archet_phrase.wav", &out, sr);
        println!("wrote /tmp/archet_phrase.wav (vels 35/60/85/110)");
    }

    /// Diagnose articulation: a quick detache note (does it sustain like a
    /// bow?) and a slurred legato pair (is it smooth?).
    ///   cargo test --release --lib engine::profile::detache_legato -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn detache_legato() {
        let sr = 48_000.0_f32;
        let block = 256usize;
        let render = |evs: Vec<(f32, ArchetCommand)>, secs: f32| -> Vec<f32> {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            tx.send(ArchetCommand::LoadPatch(Box::new(ArchetPatch::violin()))).unwrap();
            let mut out = vec![0.0f32; 0];
            let mut buf = vec![0.0f32; block * 2];
            let total = (secs * sr) as usize;
            let mut ei = 0;
            while out.len() < total {
                let t = out.len() as f32 / sr;
                while ei < evs.len() && evs[ei].0 <= t { tx.send(evs[ei].1.clone()).unwrap(); ei += 1; }
                buf.fill(0.0); eng.process_audio(&mut buf, 2);
                for i in 0..block { out.push(buf[i * 2]); }
            }
            out
        };
        let env = |o: &[f32]| -> Vec<f32> {
            let w = (0.004 * sr) as usize;
            (0..o.len()).step_by(w).map(|i| {
                let e = o[i..(i + w).min(o.len())].iter().fold(0.0f32, |m, &x| m.max(x.abs()));
                e
            }).collect()
        };
        // Detache sequence: 4 fast notes via bow changes (continuous bow, no
        // lift). The level should stay up between notes, not dip to silence
        // (which would be a string of plucks).
        let det = render(vec![
            (0.0,  ArchetCommand::NoteOn(67, 95)),
            (0.12, ArchetCommand::BowChange(69, 95)),
            (0.24, ArchetCommand::BowChange(71, 95)),
            (0.36, ArchetCommand::BowChange(72, 95)),
            (0.50, ArchetCommand::NoteOff(72)),
        ], 0.8);
        let de = env(&det);
        let dpk = de.iter().cloned().fold(0.0, f32::max).max(1e-9);
        // inter-note minimum (the dip between notes) as % of peak: high = continuous
        let span = (0.02 * sr / (0.004 * sr)) as usize;
        let mins: Vec<f32> = [0.12f32, 0.24, 0.36].iter().map(|&t| {
            let c = (t * sr / (0.004 * sr)) as usize;
            de[c.saturating_sub(span)..(c + span).min(de.len())].iter().cloned().fold(9.9, f32::min)
        }).collect();
        println!("DETACHE seq (bow changes): inter-note dips = {} % of peak (high=continuous, ~0=plucks)",
                 mins.iter().map(|m| format!("{:.0}", m / dpk * 100.0)).collect::<Vec<_>>().join(","));
        print!("  env(per 4 ms): "); for v in de.iter().take(140) { print!("{}", (v / dpk * 9.0) as u8); } println!();
        write_wav("/tmp/archet_detache.wav", &det, sr);
        // LEGATO: 67 for 150ms, slur to 69, hold 150ms, off
        let leg = render(vec![(0.0, ArchetCommand::NoteOn(67, 90)), (0.15, ArchetCommand::NoteOnLegato(69, 90)),
                              (0.30, ArchetCommand::NoteOff(69))], 0.7);
        let le = env(&leg);
        let lpk = le.iter().cloned().fold(0.0, f32::max).max(1e-9);
        // dip at the slur point (150ms): min env in 140-170ms vs surrounding
        let si = (0.15 * sr / (0.004 * sr)) as usize;
        let dip = le[si.saturating_sub(3)..(si + 8).min(le.len())].iter().cloned().fold(9.9, f32::min);
        println!("LEGATO 67->69 slur: env dip at slur = {:.0}% of peak (100%=seamless, low=gap/click)",
                 dip / lpk * 100.0);
        print!("  env(per 4 ms): "); for v in le.iter().take(80) { print!("{}", (v / lpk * 9.0) as u8); } println!();
        write_wav("/tmp/archet_legato.wav", &leg, sr);

        // Gesture arch: one short detache note (110 ms) must be an arch, its
        // peak in the middle 20-75% of the sounding span.
        let arch_note = |t0: f32| -> Vec<f32> {
            render(vec![(0.05, ArchetCommand::NoteOn(71, 90 + (t0 * 10.0) as u8)),
                        (0.05 + 0.11, ArchetCommand::NoteOff(71))], 0.5)
        };
        let a1 = arch_note(0.0);
        let e1 = env(&a1);
        let pk1 = e1.iter().cloned().fold(0.0, f32::max).max(1e-9);
        let i0 = e1.iter().position(|&v| v > 0.1 * pk1).unwrap_or(0);
        let i1 = e1.len() - 1 - e1.iter().rev().position(|&v| v > 0.1 * pk1).unwrap_or(0);
        let ipk = e1.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        let peak_pos = (ipk - i0) as f32 / ((i1 - i0).max(1)) as f32;
        // flatness: fraction of the span within 1.5 dB of peak (rectangle ~1.0, arch low)
        let flat = e1[i0..=i1].iter().filter(|&&v| v > pk1 * 0.84).count() as f32
            / ((i1 - i0 + 1) as f32);
        println!("ARCH short note: peak at {:.0}% of span (want 20-75), flat-top {:.0}% (want <50)",
                 peak_pos * 100.0, flat * 100.0);
        print!("  env(per 4 ms): "); for v in e1.iter().take(60) { print!("{}", (v / pk1 * 9.0) as u8); } println!();

        // Per-note variation: two identical-pitch/velocity notes from the same engine
        // must have different envelopes (correlation < 0.98).
        let two = render(vec![(0.05, ArchetCommand::NoteOn(71, 90)), (0.16, ArchetCommand::NoteOff(71)),
                              (0.30, ArchetCommand::NoteOn(71, 90)), (0.41, ArchetCommand::NoteOff(71))], 0.7);
        let te = env(&two);
        let seg = |t: f32| -> Vec<f32> {
            let s = (t * sr / (0.004 * sr)) as usize;
            te[s..(s + 45).min(te.len())].to_vec()
        };
        let (x, y) = (seg(0.05), seg(0.30));
        let n = x.len().min(y.len()) as f32;
        let (mx, my) = (x.iter().sum::<f32>() / n, y.iter().sum::<f32>() / n);
        let cov: f32 = x.iter().zip(&y).map(|(a, b)| (a - mx) * (b - my)).sum();
        let (vx, vy): (f32, f32) = (x.iter().map(|a| (a - mx).powi(2)).sum(),
                                     y.iter().map(|b| (b - my).powi(2)).sum());
        println!("VARIATION twin notes: envelope correlation = {:.3} (want < 0.98)",
                 cov / (vx * vy).sqrt().max(1e-9));

        // Long note (1.5 s): a held sustain, then the shaped fall.
        let lng = render(vec![(0.05, ArchetCommand::NoteOn(67, 85)), (1.55, ArchetCommand::NoteOff(67))], 2.2);
        let ln = env(&lng);
        let lpk2 = ln.iter().cloned().fold(0.0, f32::max).max(1e-9);
        let at = |t: f32| ln[((t * sr) / (0.004 * sr)) as usize] / lpk2;
        println!("LONG note env @0.1/0.4/0.9/1.4s: {:.2}/{:.2}/{:.2}/{:.2} (must vary, not flat 1.0)",
                 at(0.15), at(0.45), at(0.95), at(1.45));
        // fall time: from note_off (1.55) to 10% of peak
        let offi = ((1.55 * sr) / (0.004 * sr)) as usize;
        let fall = ln[offi..].iter().position(|&v| v < 0.1 * lpk2).map(|x| x as f32 * 4.0).unwrap_or(999.0);
        println!("LONG note shaped fall to 10% = {:.0} ms (want ~60-160, not <20=abrupt)", fall);
        write_wav("/tmp/archet_long.wav", &lng, sr);
    }


    /// The worst case for a pizzicato: repeated 16ths on one pitch. Wants
    /// every onset distinct (no choking from the predecessor's note-off) and
    /// no smear buildup.
    ///   cargo test --release --lib engine::profile::pluck_repeat -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn pluck_repeat() {
        let sr = 48_000.0_f32;
        let block = 256usize;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let pizz = ArchetPatch { pluck: true, vib_depth: 0.0, bow_noise: 0.0, release: 0.08,
                                 ..ArchetPatch::violin() };
        tx.send(ArchetCommand::LoadPatch(Box::new(pizz))).unwrap();
        let mut evs: Vec<(f32, ArchetCommand)> = Vec::new();
        for i in 0..8 {
            let t = 0.1 + i as f32 * 0.11; // ~16ths at 136 bpm
            evs.push((t, ArchetCommand::NoteOn(62, 100)));
            evs.push((t + 0.12, ArchetCommand::NoteOff(62))); // overlaps the next on!
        }
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        let mut ei = 0;
        while out.len() < (2.0 * sr) as usize {
            let t = out.len() as f32 / sr;
            while ei < evs.len() && evs[ei].0 <= t { tx.send(evs[ei].1.clone()).unwrap(); ei += 1; }
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            for i in 0..block { out.push(buf[i * 2]); }
        }
        // per-note peak: each of the 8 onsets must reach a comparable level
        let mut peaks = Vec::new();
        for i in 0..8 {
            let s = ((0.1 + i as f32 * 0.11) * sr) as usize;
            let e = (s + (0.09 * sr) as usize).min(out.len());
            peaks.push(out[s..e].iter().fold(0.0f32, |m, &x| m.max(x.abs())));
        }
        let pmax = peaks.iter().cloned().fold(0.0, f32::max).max(1e-9);
        println!("REPEAT 8x16ths peaks (rel to max, want all > 0.5): {}",
                 peaks.iter().map(|p| format!("{:.2}", p / pmax)).collect::<Vec<_>>().join(" "));
        write_wav("/tmp/archet_repeat.wav", &out, sr);
        println!("wrote /tmp/archet_repeat.wav");
    }


    /// Full-range fit: render Archet for each instrument (violin/viola/cello/bass)
    /// across its real range -> /tmp/archetf_<inst>_<pitch>.wav, so each register
    /// can be measured for the body's published band targets.
    ///   cargo test --lib engine::profile::full_range_fit -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn full_range_fit() {
        let sr = 48_000.0_f32;
        type Spec = (&'static str, fn() -> ArchetPatch, &'static [u8]);
        let specs: [Spec; 4] = [
            ("violin", ArchetPatch::violin, &[55, 62, 69, 76, 81, 88, 93]),
            ("viola", ArchetPatch::viola, &[48, 55, 62, 69, 76, 81]),
            ("cello", ArchetPatch::cello, &[36, 43, 48, 55, 60, 67]),
            ("contrabass", ArchetPatch::double_bass, &[28, 33, 40, 45, 52, 55]),
        ];
        for (name, mk, pitches) in specs {
            for &pitch in pitches {
                let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
                let mut p = mk();
                p.polyphony = 1;
                tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
                tx.send(ArchetCommand::NoteOn(pitch, 100)).unwrap();
                let block = 512usize;
                let n = (2.2 * sr) as usize / block;
                let mut out: Vec<f32> = Vec::new();
                let mut buf = vec![0.0f32; block * 2];
                for _ in 0..n { buf.fill(0.0); eng.process_audio(&mut buf, 2); for i in 0..block { out.push(buf[i*2]); } }
                write_wav(&format!("/tmp/archetf_{}_{}.wav", name, pitch), &out, sr);
            }
        }
        println!("wrote /tmp/archetf_<inst>_<pitch>.wav for violin/viola/cello/contrabass");
    }

    /// The double bass alone at its low pitches -> /tmp/archet_bass_<pitch>.wav
    ///   cargo test --lib engine::profile::dump_bass -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic - run with --ignored"]
    fn dump_bass() {
        let sr = 48_000.0_f32;
        for pitch in [28u8, 33, 40, 45, 52] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = ArchetPatch::double_bass();
            p.polyphony = 1;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(pitch, 100)).unwrap();
            let block = 512usize;
            let n = (2.2 * sr) as usize / block;
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..n { buf.fill(0.0); eng.process_audio(&mut buf, 2); for i in 0..block { out.push(buf[i*2]); } }
            write_wav(&format!("/tmp/archet_bass_{}.wav", pitch), &out, sr);
        }
        println!("wrote /tmp/archet_bass_{{28,33,40,45,52}}.wav");
    }

    fn write_wav(path: &str, mono: &[f32], sr: f32) {
        use std::io::Write;
        let n = mono.len();
        let byte_rate = (sr as u32) * 2 * 2;
        let data_len = (n * 2 * 2) as u32;
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(b"RIFF").unwrap();
        f.write_all(&(36 + data_len).to_le_bytes()).unwrap();
        f.write_all(b"WAVE").unwrap();
        f.write_all(b"fmt ").unwrap();
        f.write_all(&16u32.to_le_bytes()).unwrap();
        f.write_all(&1u16.to_le_bytes()).unwrap();
        f.write_all(&2u16.to_le_bytes()).unwrap();
        f.write_all(&(sr as u32).to_le_bytes()).unwrap();
        f.write_all(&byte_rate.to_le_bytes()).unwrap();
        f.write_all(&4u16.to_le_bytes()).unwrap();
        f.write_all(&16u16.to_le_bytes()).unwrap();
        f.write_all(b"data").unwrap();
        f.write_all(&data_len.to_le_bytes()).unwrap();
        for &s in mono {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            f.write_all(&v.to_le_bytes()).unwrap();
            f.write_all(&v.to_le_bytes()).unwrap();
        }
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    fn render_blocks(eng: &mut ArchetEngine, blocks: usize, block: usize) -> Vec<f32> {
        let mut mono = Vec::with_capacity(blocks * block);
        let mut buf = vec![0.0f32; block * 2];
        for _ in 0..blocks {
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            for fr in buf.chunks(2) { mono.push(fr[0]); }
        }
        mono
    }

    /// Preset-sweep regression: every factory preset must render audible,
    /// bounded output when bowed. Catches presets left silent by a bad
    /// param combo (the class the guitar sweep once found 53 of).
    #[test]
    fn all_factory_presets_are_audible() {
        let sr = 48_000.0f32;
        for p in ArchetPatch::factory_presets() {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let name = p.name.clone();
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(60, 100)).unwrap();
            let mono = render_blocks(&mut eng, 80, 512); // ~0.85 s
            let peak = mono.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 1e-3, "preset '{name}' inaudible (peak {peak})");
            assert!(peak < 8.0, "preset '{name}' blew up (peak {peak})");
        }
    }

    /// output_level is applied once, in the engine's voice-sum norm: halving
    /// the level halves the output.
    #[test]
    fn output_level_is_applied_once() {
        let sr = 48_000.0f32;
        let render_peak = |level: f32| -> f32 {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = ArchetPatch::violin();
            p.polyphony = 1;
            p.ensemble = 1.0;
            p.output_level = level;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(69, 100)).unwrap();
            let mono = render_blocks(&mut eng, 100, 512); // ~1 s
            mono.iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let full = render_peak(1.0);
        let half = render_peak(0.5);
        assert!(full > 1e-3, "violin render should be audible (peak {full})");
        let ratio = half / full.max(1e-9);
        assert!((ratio - 0.5).abs() < 0.1,
            "output_level 0.5 should scale output by ~0.5 (got ratio {ratio}; \
             ~0.25 means the level is applied twice again)");
    }

    /// Stealing a sounding voice fades the old sound over about 3 ms before
    /// the fresh attack starts.
    #[test]
    fn voice_steal_fades_instead_of_slamming() {
        let sr = 48_000.0f32;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let mut p = ArchetPatch::violin();
        p.polyphony = 1;
        p.ensemble = 1.0;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        tx.send(ArchetCommand::NoteOn(62, 110)).unwrap();
        // Establish a loud sustained tone.
        let pre = render_blocks(&mut eng, 750, 64); // ~1 s at block 64
        let pre_tail = &pre[pre.len() - 240..];      // last 5 ms
        let pre_rms = (pre_tail.iter().map(|s| s * s).sum::<f32>()
            / pre_tail.len() as f32).sqrt();
        assert!(pre_rms > 1e-3, "note should be sounding before the steal (rms {pre_rms})");

        // Steal the only voice with a new pitch.
        tx.send(ArchetCommand::NoteOn(74, 110)).unwrap();
        let post = render_blocks(&mut eng, 4, 64);   // first ~5.3 ms after the steal
        let fade_window = &post[..144];              // the 3 ms declick fade
        let post_rms = (fade_window.iter().map(|s| s * s).sum::<f32>()
            / fade_window.len() as f32).sqrt();
        // With the fade the 3 ms window still carries the old note (a
        // linear fade 1 -> 0, rms about 0.58x); an instant steal would
        // render near-silence there.
        assert!(post_rms > pre_rms * 0.25,
            "steal slammed the old sound to silence (pre rms {pre_rms}, fade-window rms {post_rms})");

        // And the stolen-to note must actually speak afterwards.
        let after = render_blocks(&mut eng, 300, 64); // ~0.4 s
        let peak = after.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 1e-3, "the queued steal note never sounded (peak {peak})");
    }

    /// LoadPatch clamps the polyphony to the voice pool.
    #[test]
    fn load_patch_clamps_polyphony() {
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(48_000.0);
        let mut p = ArchetPatch::violin();
        p.polyphony = 48;
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        let mut buf = vec![0.0f32; 128];
        eng.process_audio(&mut buf, 2);
        assert_eq!(eng.patch.polyphony, MAX_VOICES as u8);
    }

    /// A patch push must not re-seed an ensemble desk that is already at that
    /// offset. The editor has no per-parameter commands, so it pushes the whole
    /// patch on every knob edit; re-seeding unconditionally re-randomised the
    /// voices of a sounding desk on every drag tick.
    #[test]
    fn repeated_patch_pushes_do_not_reseed_an_unchanged_ensemble_desk() {
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(48_000.0);
        let mut p = crate::patch::ArchetPatch { seed_offset: 7, ..Default::default() };
        let _ = tx.send(ArchetCommand::LoadPatch(Box::new(p.clone())));
        let mut buf = vec![0.0f32; 128];
        eng.process_audio(&mut buf, 2);
        assert_eq!(eng.applied_seed, 7, "the first load must apply the offset");

        // Pushing the same patch again (what a knob drag does) must be a no-op
        // for seeding, and changing the offset must still take effect.
        let _ = tx.send(ArchetCommand::LoadPatch(Box::new(p.clone())));
        eng.process_audio(&mut buf, 2);
        assert_eq!(eng.applied_seed, 7);

        p.seed_offset = 9;
        let _ = tx.send(ArchetCommand::LoadPatch(Box::new(p)));
        eng.process_audio(&mut buf, 2);
        assert_eq!(eng.applied_seed, 9, "a real change must still re-seed");
    }
}

/// The extraction's audio guarantee.
///
/// Hashes a fixed render of six factory presets spanning every articulation the
/// model has (solo arco, expressive arco, section cluster, full-range composite
/// desk, pizzicato).
///
/// Archet is deterministic by construction: every noise stream is a private
/// xorshift seeded from a constant (`ArchetString::rng = 0x1234_5678`,
/// per-voice `ArchetVoice::new(sr, i)`, `SympStrings::new(sr, 0)`), the engine
/// constructor spawns no thread and touches no lazily-warmed shared bank, and
/// nothing reads the clock. So a plain render hashes stably.
#[cfg(test)]
mod golden_audio {
    use super::*;

    /// A pizzicato rings by the string's own losses for as long as the key is
    /// held, and the hand mutes it once the key is up, at the release
    /// control's pace. The player who lets a note ring holds it; the player
    /// in a fast passage does not, and the strings would pile up into mud if
    /// nothing but the string's own losses ended them.
    #[test]
    fn a_pizzicato_rings_while_held_and_the_hand_mutes_it_after() {
        let sr = 48_000.0f32;
        let bank = super::super::patch::ArchetPatch::factory_presets();
        let preset = bank
            .iter()
            .find(|p| p.name == "Cello Pizzicato")
            .expect("the bank no longer has Cello Pizzicato");
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        let _ = tx.send(ArchetCommand::LoadPatch(Box::new(preset.clone())));
        let _ = tx.send(ArchetCommand::NoteOn(50, 100));
        let block = 512usize;
        let mut buf = vec![0.0f32; block * 2];
        let level = |eng: &mut ArchetEngine, buf: &mut Vec<f32>, blocks: usize| -> f32 {
            let mut acc = 0.0f64;
            let mut n = 0usize;
            for _ in 0..blocks {
                buf.fill(0.0);
                eng.process_audio(buf, 2);
                for s in buf.iter() {
                    acc += (*s as f64) * (*s as f64);
                    n += 1;
                }
            }
            (acc / n as f64).sqrt() as f32
        };
        let struck = level(&mut eng, &mut buf, 20);
        // Held for another quarter second: the string rings on by itself.
        let held = level(&mut eng, &mut buf, 24);
        let _ = tx.send(ArchetCommand::NoteOff(50));
        // The hand mutes at the release control's pace, so the fade itself
        // is skipped and the level read a fifth of a second on, where the
        // note must be long gone.
        let _ = level(&mut eng, &mut buf, 19);
        let muted = level(&mut eng, &mut buf, 12);
        assert!(struck > 1e-4, "the pizzicato never spoke: {struck}");
        assert!(
            held > struck * 0.3,
            "the note died while the key was held: {held} against {struck}"
        );
        assert!(
            muted < held * 0.05,
            "the hand did not mute the note after the key came up: {muted} against {held} held"
        );
    }

    #[test]
    fn the_rendered_audio_is_bit_identical() {
        let sr = 48_000.0f32;
        let bank = super::super::patch::ArchetPatch::factory_presets();
        let block = 256usize;
        let mut left: Vec<f32> = Vec::new();
        // By name, not by index. An index says nothing about what it covers,
        // so a preset removed or inserted ahead of one silently changes what
        // this gate is watching; a name that is gone fails here and says so.
        for want in [
            "Violin Solo",
            "Baroque Violin",
            "Violin Section",
            "Cinematic Strings",
            "Cello Pizzicato",
            "Pizzicato Section",
        ] {
            let preset = bank
                .iter()
                .find(|p| p.name == want)
                .unwrap_or_else(|| panic!("the bank no longer has {want}"));
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let _ = tx.send(ArchetCommand::LoadPatch(Box::new(preset.clone())));
            let _ = tx.send(ArchetCommand::NoteOn(62, 100));
            let mut buf = vec![0.0f32; block * 2];
            for b in 0..40 {
                if b == 24 { let _ = tx.send(ArchetCommand::NoteOff(62)); }
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                left.extend(buf.iter().step_by(2));
            }
        }
        let h = crate::fingerprint::of(&left);
        eprintln!("GOLDEN = {h:#018x}");
        assert_eq!(h, 0x30ee3f2e8689e16c, "the engine's rendered audio changed");
    }
}

#[cfg(test)]
mod fader {
    use super::*;

    fn render(patch: ArchetPatch, chord: &[u8], secs: f32) -> Vec<f32> {
        let sr = 48_000.0f32;
        let block = 512usize;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        tx.send(ArchetCommand::LoadPatch(Box::new(patch))).unwrap();
        for &n in chord {
            tx.send(ArchetCommand::NoteOn(n, 127)).unwrap();
        }
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; block * 2];
        for _ in 0..((secs * sr) as usize / block) {
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            out.extend_from_slice(&buf);
        }
        out
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// The largest section holding a chord at full velocity with the
    /// level control at its top stays at full scale, within the slope's
    /// residual, and the fader is what holds it.
    #[test]
    fn a_section_chord_is_held_at_full_scale() {
        let mut p = ArchetPatch::violin();
        p.ensemble = PLAYERS_MAX;
        p.polyphony = 32;
        p.output_level = 4.0;
        let out = render(p, &[55, 62, 67, 74], 1.5);
        let settled = &out[(1.0 * 48_000.0 * 2.0) as usize..];
        assert!(peak(settled) <= 1.02, "past full scale: {}", peak(settled));
        assert!(peak(settled) > 0.8, "held far under full scale: {}", peak(settled));
    }

    /// The loudest the bank holds, the largest section holding the
    /// fullest chord its pool lights at full velocity, peaks just under
    /// full scale on the engine's own scale, the fader idle: that is what
    /// the bow level is set to.
    #[test]
    fn the_scale_is_the_largest_section() {
        let sr = 48_000.0f32;
        let block = 512usize;
        let mut p = ArchetPatch::violin();
        p.ensemble = PLAYERS_MAX;
        p.polyphony = 32;
        let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
        tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
        let mut buf = vec![0.0f32; block * 2];
        eng.process_audio(&mut buf, 2);
        let notes = MAX_VOICES / eng.voices_per_note();
        for (i, n) in [55u8, 62, 67, 74, 79, 86, 91, 98].iter().enumerate() {
            if i < notes {
                tx.send(ArchetCommand::NoteOn(*n, 127)).unwrap();
            }
        }
        let mut top = 0.0f32;
        for i in 0..((1.6 * sr) as usize / block) {
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            assert_eq!(eng.fader_gain(), 1.0, "the fader worked on the engine's own scale");
            if i * block > (0.4 * sr) as usize {
                top = top.max(peak(&buf));
            }
        }
        let db = 20.0 * top.log10();
        assert!((-3.0..=0.0).contains(&db), "the largest section's chord peaks at {db:.1} dBFS");
    }

    /// A soloist's note stays under full scale on its own headroom, so
    /// the fader leaves it alone: its gain is one throughout.
    #[test]
    fn a_soloist_is_not_touched() {
        let mut f = Fader::new(48_000.0);
        f.hold_for(196.0, 48_000.0);
        let out = render(ArchetPatch::violin(), &[69], 1.0);
        assert!(peak(&out) < 1.0);
        for pair in out.chunks(2) {
            assert_eq!(f.gain(pair[0].abs().max(pair[1].abs())), 1.0);
        }
    }

    /// The held peak is kept for the hold and then falls at the return
    /// pace: after one second the gain has come back six decibels.
    #[test]
    fn the_gain_comes_back_at_a_walking_pace() {
        let sr = 48_000.0f32;
        let mut f = Fader::new(sr);
        f.hold_for(41.2, sr);
        let hold = f.hold;
        let mut g = f.gain(4.0);
        for _ in 0..hold {
            g = f.gain(0.0);
        }
        assert!((g - 0.25).abs() < 1e-2, "the slope did not reach the target within the hold: {g}");
        for _ in 0..(sr as usize) {
            f.gain(0.0);
        }
        let g = f.gain(0.0);
        assert!((20.0 * (g / 0.25).log10() - Fader::RETURN_DB_PER_S).abs() < 0.05, "gain {g}");
    }
}
