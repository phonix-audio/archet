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

        let norm = (1.0 / (poly as f32).sqrt()) * self.patch.output_level;
        // diffuser fill = how far the requested section exceeds the real
        // voice pool (8 -> 0, ~40+ -> 1, saturating). O(1) regardless of size.
        let fill = if self.patch.ensemble > 8.0 {
            ((self.patch.ensemble - 8.0) / 60.0).clamp(0.0, 1.0)
        } else { 0.0 };
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
        const PHYS_CAP: usize = 8; // perceptual-saturation pool; size > 8 = chorus fill
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
        const GOLDEN: u64 = 0x687c32d0e7e20ffe; // pre-WAVE-3 render (voice/frame reorder must match)
        assert_eq!(h, GOLDEN, "Archet ensemble render drifted from golden (hash {h:#018x})");
    }
}

#[cfg(test)]
mod profile {
    use super::*;

    /// Acoustic-fit harness: render the violin patch at G3/D4/A4/D5/A5 (the same
    /// notes as the timidity FluidR3 reference) -> /tmp/archet_<pitch>.wav, so a
    /// Python script can measure the harmonic-envelope distance to the real violin
    /// and drive the body/string tuning OBJECTIVELY (no listening).
    ///   cargo test --lib archet::engine::profile::violin_fit -- --ignored --nocapture
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
    ///   cargo test --lib archet::engine::profile::phrase_dyn -- --ignored --nocapture
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
    ///   cargo test --release --lib archet::engine::profile::detache_legato -- --ignored --nocapture
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
    ///   cargo test --release --lib archet::engine::profile::pluck_repeat -- --ignored --nocapture
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
    /// across its real range -> /tmp/archetf_<inst>_<pitch>.wav, to compare against
    /// the timidity FluidR3 references (GM 40/41/42/43) per instrument per register.
    ///   cargo test --lib archet::engine::profile::full_range_fit -- --ignored --nocapture
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

    /// Isolate the Archet DOUBLE BASS at real low pitches so we can compare it to
    /// the real contrabass (timidity GM 43) and see if it behaves like a bowed
    /// string or a synth saw. -> /tmp/archet_bass_<pitch>.wav
    ///   cargo test --lib archet::engine::profile::dump_bass -- --ignored --nocapture
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
    ///   cargo test --lib archet::engine::profile::bow_derisk -- --ignored --nocapture
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

    /// A pizzicato rings on after the finger leaves the string. It was a
    /// harpsichord before: a damper fell on key release and stopped the tone
    /// in forty milliseconds, which is what a keyboard does and what a
    /// violinist does not.
    #[test]
    fn a_pizzicato_rings_on_after_the_key_is_released() {
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
        let _ = tx.send(ArchetCommand::NoteOff(50));
        // A quarter second after the finger leaves, and again half a second on.
        let just_after = level(&mut eng, &mut buf, 24);
        let later = level(&mut eng, &mut buf, 48);
        assert!(struck > 1e-4, "the pizzicato never spoke: {struck}");
        assert!(
            just_after > struck * 0.15,
            "the note stopped with the key: {just_after} against {struck} while held"
        );
        assert!(later < struck, "the note did not decay at all: {later} against {struck}");
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
        assert_eq!(h, 0xd4ab_701d_b719_cdb1, "the engine's rendered audio changed");
    }
}
