//! ArchetEngine — polyphonic bowed-string voice allocator and audio loop.
//!
//! Mirrors Strata's offline-driving interface (`new_for_plugin`, command queue,
//! interleaved `process_audio`) so the summer_storm render harness drives it the
//! same way as every other engine.

use std::sync::mpsc;

use phonix_rt::{meter_channel, SharedReader, Writer};
use super::patch::{ArchetParam, ArchetPatch};
use super::sympathetic::SympStrings;
use super::voice::{ArchetVoice, MAX_VOICES};

/// Physical voices a unison ever lights, whatever section size is asked for:
/// the perceptual-saturation pool (Ternstroem), enough independent
/// instantaneous pitches to fill the scatter band. Larger sections are
/// realised by the O(1) section diffuser instead, so the cost stays flat.
/// Both `fire_unison` and the voice-sum norm need it: one to spend the
/// voices, the other to divide by what a note actually lights.
const PHYS_CAP: usize = 8;

#[derive(Debug, Clone)]
pub enum ArchetCommand {
    NoteOn(u8, u8),
    /// Same-string legato: retune the currently-bowing voice instead of starting
    /// a fresh bow stroke (slurred notes connect without re-attacking).
    NoteOnLegato(u8, u8),
    /// Détaché bow change: reverse the bow on the currently-bowing voice (continuous
    /// contact, no lift -> no pluck) for a separate but connected note.
    BowChange(u8, u8),
    NoteOff(u8),
    AllNotesOff,
    PitchBend(f32),
    SetPolyphony(u8),
    SetOutputLevel(f32),
    LoadPatch(Box<ArchetPatch>),
    /// Set ONE named field of the patch.
    ///
    /// Added so the editor stops pushing a whole patch on every knob turn.
    /// `LoadPatch` rebuilds the sympathetic open strings and walks the voice
    /// pool to re-seed it; the two guards below (`inst != self.symp_inst` and
    /// `seed_offset != self.applied_seed`) exist ONLY because a drag frame used
    /// to arrive as a full patch. Naming the field is the actual fix.
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

pub struct ArchetEngine {
    voices: Vec<ArchetVoice>,
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
    patch_dirty: bool,
    // Sympathetic open-string bank (the fine-instrument "ring"): persists across
    // notes so runs leave a glowing halo. Rebuilt when the patch instrument changes.
    symp: SympStrings,
    symp_inst: usize,
    /// Seed offset already applied to the voices. Re-seeding is idempotent only
    /// in the sense that it always produces the SAME decorrelation for a given
    /// offset, so doing it on every patch load perturbs a sounding ensemble
    /// desk each time the editor pushes a patch (which it does per knob edit).
    applied_seed: u32,
    symp_level: f32,
    note_seq: u64, // monotonic note-on counter (oldest-voice release)
    // string-SECTION diffuser: fills toward the large-N texture above the
    // bounded real-voice pool (O(1), see section.rs). Bypassed when small.
    section: crate::section::SectionDiffuser,
}

impl ArchetEngine {
    pub fn new(
        sample_rate: f32,
        command_rx: mpsc::Receiver<ArchetCommand>,
        meter_writer: Writer<ArchetMeterState>,
    ) -> Self {
        Self {
            voices: (0..MAX_VOICES).map(|i| ArchetVoice::new(sample_rate, i)).collect(),
            held_keys: Vec::new(),
            patch: ArchetPatch::default(),
            command_rx,
            meter_writer,
            meter_shadow: ArchetMeterState::default(),
            sample_rate,
            meter_counter: 0,
            peak_l: 0.0,
            peak_r: 0.0,
            patch_dirty: true,
            symp: SympStrings::new(sample_rate, 0),
            symp_inst: 0,
            applied_seed: 0,
            symp_level: 0.015,
            note_seq: 0,
            section: crate::section::SectionDiffuser::new(sample_rate),
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
        self.symp = SympStrings::new(sample_rate, self.symp_inst);
        self.section = crate::section::SectionDiffuser::new(sample_rate);
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
        let frames = output.len() / channels.max(1);
        self.process_commands();

        let poly = (self.patch.polyphony as usize).min(MAX_VOICES);
        let any_active = self.voices[..poly].iter().any(|v| v.is_active());

        // keep rendering while the sympathetic open strings still ring (the
        // halo must not cut off at rests when all voices go idle)
        let tail_active = self.symp.active();
        if !any_active && !tail_active {
            for s in output.iter_mut() {
                *s = 0.0;
            }
            self.publish_meter(frames, 0);
            return;
        }

        // Two terms, and neither is the polyphony setting. That setting is how
        // much overlap the player is ALLOWED, so keying the norm to it made the
        // instrument quieter for raising a limit, and cut a desk for voices it
        // could never light.
        //
        // The first term is a fixed headroom for the notes a player sounds at
        // once. It cannot follow the live count, which would make a desk pump
        // as notes enter and leave, and it cannot be dropped: without it two
        // notes already reach full scale. A chord of this many notes is what
        // the instrument is voiced to hold.
        //
        // The second is physical: one note of a section lights that many
        // voices, they sum incoherently, and dividing by the root of their
        // number puts a section note back beside a solo one.
        const CHORD: f32 = 8.0;
        let lit = if self.patch.ensemble >= 1.5 {
            (self.patch.ensemble.max(2.0).round() as usize).clamp(2, PHYS_CAP)
        } else {
            1
        };
        let norm = (1.0 / (CHORD * lit as f32).sqrt()) * self.patch.output_level;
        // diffuser fill = how far the requested section exceeds the real
        // voice pool (8 -> 0, ~40+ -> 1, saturating). O(1) regardless of size.
        let fill = if self.patch.ensemble > 8.0 {
            ((self.patch.ensemble - 8.0) / 60.0).clamp(0.0, 1.0)
        } else { 0.0 };
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
        for frame_idx in 0..frames {
            // Per-voice constant-power panning: in ENSEMBLE mode each unison
            // player is seated at its own azimuth (set at note-on), so the
            // section spreads across the stage. Non-ensemble voices have
            // pan 0 -> both channels equal -> identical to the old mono dup.
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
            // GLOBAL resonance: driven by the mono sum, added centered.
            let tail = self.symp.process(mono) * self.symp_level;
            // large-section fill: diffuse the (dry) voice field toward the
            // continuous many-player texture; the tail stays clean + centered.
            let (dl, dr) = if fill > 0.0 {
                self.section.process(sl, sr_, fill)
            } else {
                (sl, sr_)
            };
            let l = (dl + tail).clamp(-1.0, 1.0);
            let r = (dr + tail).clamp(-1.0, 1.0);

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
        self.publish_meter(frames, vc);
    }

    fn publish_meter(&mut self, frames: usize, voice_count: usize) {
        self.meter_counter += frames;
        let update_interval = (self.sample_rate / 30.0) as usize;
        if self.meter_counter >= update_interval {
            self.meter_counter = 0;
            self.meter_shadow.peak_l = self.peak_l;
            self.meter_shadow.peak_r = self.peak_r;
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
                        let poly = (self.patch.polyphony as usize).min(MAX_VOICES);
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
                    // Resurrect the loudest still-sounding voice (the bow that's
                    // down, even if it was just note_off'd) and retune it -> the
                    // slur continues. Robust: every note still gets a NoteOff, so
                    // no voice can be stranded/held forever. No sounding voice ->
                    // a fresh bow stroke.
                    let poly = (self.patch.polyphony as usize).min(MAX_VOICES);
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
                    // Détaché: reverse the bow on the still-bowing voice (no lift -> no
                    // pluck). Same voice-selection as legato; fresh stroke if none down.
                    let poly = (self.patch.polyphony as usize).min(MAX_VOICES);
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
                    // Release the OLDEST held CLUSTER of this pitch (one off pairs
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
        // Patches saved by the old 1..48 GUI knob (or hand-edited JSON)
        // can carry a polyphony above the real voice pool — clamp so the
        // GUI mirror and the engine agree.
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
        // per-desk decorrelation for string sections: re-seed all voices so
        // this engine instance is independent of others.
        // Only when the offset actually CHANGES. The editor pushes a whole
        // patch on every knob edit (this engine exposes no per-parameter
        // commands), so re-seeding unconditionally re-randomised a sounding
        // ensemble desk on every drag tick.
        if self.patch.seed_offset != 0 && self.patch.seed_offset != self.applied_seed {
            let off = self.patch.seed_offset;
            for v in &mut self.voices { v.reseed(off); }
            self.applied_seed = off;
        }
    }

    /// ENSEMBLE / string-SECTION note-on (research model, Ternström JASA on
    /// unison frequency scatter + Meyer on orchestral sections): one melodic
    /// note becomes `count` REAL physical-model players, each with
    ///  - a STATIC F0 offset drawn ~ Gaussian, SD ~14 cents * depth (the
    ///    scatter that diffuses each partial into a band -> no phase-lock,
    ///    no reed); the spread scales with partial number automatically
    ///    because it is a true pitch offset;
    ///  - independent bow-noise / micro-pitch / vibrato-rate seeds (already
    ///    keyed per voice_idx) + a random vibrato phase per note = vibrato
    ///    ASYNCHRONY, the second primary section cue;
    ///  - its own stage azimuth.
    /// Bounded cost (count voices, not N engines) -> real-time in the plugin.
    fn fire_unison(&mut self, note: u8, vel: u8, seq: u64) {
        // `ensemble` is the SECTION SIZE in PLAYERS. Real physical voices
        // are bounded at PHYS_CAP (the perceptual-saturation point,
        // Ternström): enough independent instantaneous pitches to fill the
        // scatter band. Requested sizes ABOVE the cap are realized by the
        // O(1) section diffuser (engine output) -- cost stays flat, so a
        // 100-violin setting is feasible.
        let size = self.patch.ensemble.max(2.0);
        let count: usize = (size.round() as usize).clamp(2, PHYS_CAP);
        // measured inter-player F0 dispersion of a real section is 20-30 cents
        // (Cuesta/Chandna unison analysis; Ternström) -- NOT the 14c tight-
        // unison preference. 22c SD here + the per-voice slow drift gives the
        // living, partials-crossing section instead of a fused fat unison.
        let sd_cents = 22.0;
        let onset_max = (0.035 * self.sample_rate) as u32; // ~35 ms attack spread
        for k in 0..count {
            // deterministic per (note, k): sum of 3 uniforms ~ Gaussian (CLT)
            let h = (note as u32)
                .wrapping_mul(2654435761)
                .wrapping_add((k as u32).wrapping_mul(40503))
                .wrapping_add(seq as u32);
            let u = |sh: u32| ((h >> sh) & 0x3ff) as f32 / 1023.0;
            let g = (u(0) + u(10) + u(20)) / 3.0 * 2.0 - 1.0; // ~[-1,1], bell-ish
            let det = g * sd_cents * 1.7; // 1.7: map the triangular-ish range to ~SD
            // seat the players across the desk: -0.7 .. +0.7
            let pan = if count > 1 {
                ((k as f32 / (count - 1) as f32) * 2.0 - 1.0) * 0.7
            } else { 0.0 };
            // ONSET ASYNCHRONY: player 0 lands on time; the rest 0..~28 ms late
            // (bows never land together) -> spread attack, no fused transient.
            let onset = if k == 0 { 0 } else { ((h >> 5) % onset_max) as usize };
            let idx = self.allocate_voice_idx(note);
            self.voices[idx].set_unison(det, pan, onset);
            self.voices[idx].note_on(note, vel, &self.patch);
            self.voices[idx].on_seq = seq;
        }
    }

    fn allocate_voice_idx(&mut self, note: u8) -> usize {
        let poly = (self.patch.polyphony as usize).min(MAX_VOICES);
        if let Some(i) = self.voices[..poly].iter().position(|v| !v.is_active()) {
            return i;
        }
        if let Some(i) = self.voices[..poly].iter().position(|v| v.note == Some(note)) {
            return i;
        }
        0
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
            "no archet preset produced audible output — excitation path is broken");
    }

    /// Byte-identity golden for the multi-voice render path. The live-RT WAVE 3
    /// refactor reorders the per-voice / per-frame loops in process_audio for
    /// cache locality; the output MUST stay bit-identical. This exercises
    /// ensemble mode (many active voices + per-voice constant-power pan + the
    /// global sympathetic tail on the mono sum), so the voice-summation order
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
        let mut h = 0u64;
        let mut buf = vec![0.0f32; block * 2];
        for b in 0..nblocks {
            if b == nblocks * 2 / 3 {
                tx.send(ArchetCommand::NoteOff(55)).unwrap();
                tx.send(ArchetCommand::NoteOff(62)).unwrap();
            }
            buf.fill(0.0);
            eng.process_audio(&mut buf, 2);
            for &s in &buf { h = h.rotate_left(7) ^ s.to_bits() as u64; }
        }
        const GOLDEN: u64 = 0x97105677d1b4f26e; // the ensemble render, note-offs included
        assert_eq!(h, GOLDEN, "Archet ensemble render drifted from golden (hash {h:#018x})");
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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

    /// A bowed phrase, to hear whether a long release blurs the line.
    ///
    /// The control now governs how the bow leaves, and the presets were voiced
    /// when that fall was fixed and short, so the longest of them now holds a
    /// note several times longer than it used to. Notes that overlap because
    /// the previous one has not let go is the thing to listen for, and no
    /// measurement settles it.
    ///   cargo test --lib engine::profile::bow_phrase -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
        let desks: [(&[[&[u8]; 4]; 8], &[&[u8]; 4]); 5] = [
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
            let acc = if accent[bar] { 12 } else { 0 };
            let vel = (dynamic[bar] + acc + lean + VOICE[n.desk]).clamp(1, 127) as u8;
            let sounding = if piece.desks[n.desk] == "bass" { n.midi - 12 } else { n.midi };
            s.push((n.desk, on, sounding, vel, n.len, n.art));
        }
        s
    }

    /// The velocity of each bar from a set of marks, with the bars that
    /// carry an accent. Bars are 1-based; index 0 is unused.
    fn dynamics(bars: usize, marks: &[(usize, &str)]) -> (Vec<i32>, Vec<bool>) {
        const SWELL: i32 = 24;
        let level_of = |m: &str| -> Option<i32> {
            Some(match m {
                "ppp" => 36,
                "pp" => 48,
                "p" => 62,
                "mp" => 72,
                "mf" => 82,
                "f" => 92,
                "ff" => 104,
                "fff" => 116,
                _ => return None,
            })
        };
        let mut marks: Vec<(usize, &str)> = marks.to_vec();
        marks.sort_by_key(|m| m.0);
        // Level marks carry forward; a hairpin then bends the bars between
        // itself and the next mark.
        let mut dynamic = vec![62i32; bars + 2];
        let mut current = 62;
        for bar in 1..=bars {
            for &(b, m) in &marks {
                if b == bar {
                    if let Some(l) = level_of(m) {
                        current = l;
                    }
                }
            }
            dynamic[bar] = current;
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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
    #[ignore = "diagnostic — run with --ignored"]
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

    /// Diagnose articulation: a quick DETACHE note (does it sustain like a bow, or
    /// peak-then-decay like a pizzicato?) and a slurred LEGATO pair (smooth?).
    ///   cargo test --release --lib engine::profile::detache_legato -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic"]
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
        // DÉTACHÉ SEQUENCE: 4 fast notes via bow changes (continuous bow, no lift).
        // The level should stay UP between notes (continuous), not dip to silence
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
        println!("DÉTACHÉ seq (bow changes): inter-note dips = {} % of peak (high=continuous, ~0=plucks)",
                 mins.iter().map(|m| format!("{:.0}", m / dpk * 100.0)).collect::<Vec<_>>().join(","));
        print!("  env(每4ms): "); for v in de.iter().take(140) { print!("{}", (v / dpk * 9.0) as u8); } println!();
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
        print!("  env(每4ms): "); for v in le.iter().take(80) { print!("{}", (v / lpk * 9.0) as u8); } println!();
        write_wav("/tmp/archet_legato.wav", &leg, sr);

        // GESTURE ARCH: one short détaché note (110 ms) must be an ARCH (peak in the
        // middle 20-75% of the sounding span, NOT an instant-peak flat-top rectangle).
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
        print!("  env(每4ms): "); for v in e1.iter().take(60) { print!("{}", (v / pk1 * 9.0) as u8); } println!();

        // PER-NOTE VARIATION: two identical-pitch/velocity notes from the same engine
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

        // LONG NOTE (1.5 s): must LIVE -- breathing sustain + swell, then shaped fall.
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


    /// The worst case for a pizzicato: REPEATED 16ths on one pitch. Wants
    /// every onset distinct (no choking from the predecessor's note-off) and
    /// no smear buildup.
    ///   cargo test --release --lib engine::profile::pluck_repeat -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic"]
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


    /// Full-range fit: render Archet for EACH instrument (violin/viola/cello/bass)
    /// across its real range -> /tmp/archetf_<inst>_<pitch>.wav, so each register
    /// can be measured for the body's published band targets.
    ///   cargo test --lib engine::profile::full_range_fit -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic — run with --ignored"]
    fn full_range_fit() {
        let sr = 48_000.0_f32;
        let specs: [(&str, fn() -> ArchetPatch, &[u8]); 4] = [
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

    /// Isolate the Archet DOUBLE BASS at real low pitches, to hear whether it
    /// behaves like a bowed string or a synth saw. -> /tmp/archet_bass_<pitch>.wav
    ///   cargo test --lib engine::profile::dump_bass -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic — run with --ignored"]
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

    /// De-risk: render a held D4 (violin) -> /tmp/archet_bow.wav.
    ///   cargo test --lib engine::profile::bow_derisk -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic — run with --ignored"]
    fn bow_derisk() {
        let sr = 48_000.0_f32;
        // A/B the two friction models on the same held D4.
        for (kind, path) in [
            (crate::patch::FrictionKind::Static, "/tmp/archet_bow.wav"),
            (crate::patch::FrictionKind::ElastoPlastic, "/tmp/archet_bow_ep.wav"),
        ] {
            let (mut eng, tx, _mr) = ArchetEngine::new_for_plugin(sr);
            let mut p = ArchetPatch::violin();
            p.polyphony = 1;
            p.friction = kind;
            tx.send(ArchetCommand::LoadPatch(Box::new(p))).unwrap();
            tx.send(ArchetCommand::NoteOn(62, 100)).unwrap(); // D4

            let block = 512usize;
            let n_hold = (2.0 * sr) as usize / block;
            let n_tail = (0.6 * sr) as usize / block;
            let mut out: Vec<f32> = Vec::new();
            let mut buf = vec![0.0f32; block * 2];
            for _ in 0..n_hold {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block { out.push(buf[i * 2]); }
            }
            tx.send(ArchetCommand::NoteOff(62)).unwrap();
            for _ in 0..n_tail {
                buf.fill(0.0);
                eng.process_audio(&mut buf, 2);
                for i in 0..block { out.push(buf[i * 2]); }
            }
            write_wav(path, &out, sr);
            let peak = out.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
            println!("wrote {} ({:?}) peak={:.3}", path, kind, peak);
        }
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

    /// Regression (audit v2): output_level was applied BOTH per voice and
    /// in the engine's voice-sum norm, squaring the OUT knob. Halving the
    /// level must now halve the output, not quarter it.
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

    /// Regression (audit v2 MED): stealing an audibly-sounding voice used
    /// to slam amp_env to 0 + reset the modal string in one sample (a
    /// click). The steal must now fade the old sound over ~3 ms before the
    /// fresh attack starts.
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
        // With the declick fade the 3 ms window still carries the old note
        // (linear fade 1 -> 0, rms ~0.58x). A slammed steal renders
        // near-silence there (fresh modal + amp_env restarting from 0).
        assert!(post_rms > pre_rms * 0.25,
            "steal slammed the old sound to silence (pre rms {pre_rms}, fade-window rms {post_rms})");

        // And the stolen-to note must actually speak afterwards.
        let after = render_blocks(&mut eng, 300, 64); // ~0.4 s
        let peak = after.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 1e-3, "the queued steal note never sounded (peak {peak})");
    }

    /// LoadPatch must clamp polyphony to the real voice pool (the old GUI
    /// allowed 1..48 with MAX_VOICES = 32).
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
        let mut p = crate::patch::ArchetPatch::default();
        p.seed_offset = 7;
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
/// desk, pizzicato, harpsichord). Written so it compiles unchanged on both
/// sides of the split: `super::super::patch` is `crate::patch` in the
/// monolith and `crate::patch` in the extracted crate.
///
/// Archet is deterministic by construction: every noise stream is a private
/// xorshift seeded from a CONSTANT (`ArchetString::rng = 0x1234_5678`,
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
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
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
                for x in buf.iter() {
                    for byte in x.to_bits().to_le_bytes() {
                        h ^= byte as u64;
                        h = h.wrapping_mul(0x0000_0100_0000_01b3);
                    }
                }
            }
        }
        eprintln!("GOLDEN = {h:#018x}");
        assert_eq!(h, 0x6322_013e_5b5e_acf0, "the engine's rendered audio changed");
    }
}
