//! ArchetVoice — one bowed string: waveguide + friction junction + modal body,
//! driven by a bow with realistic articulation (no pluck envelope).
//!
//! NoteOn engages the bow (force ramps up over `attack`); NoteOff lifts it
//! (force ramps to 0 over `release`) and the string decays through its own
//! losses. There is no AD/ADSR amplitude envelope re-triggering the tone, which
//! is what caused the clicks/missing notes in the earlier Strata graft.

use super::body::{Biquad, ModalBody};
use super::friction::Friction;
use super::patch::ArchetPatch;
use super::string::BowedWaveguide;
use super::modal::ModalString;

pub const MAX_VOICES: usize = 32;
const CTRL_INTERVAL: usize = 32;

/// Small, fast white-noise source for bow scratch.
#[derive(Debug, Clone)]
struct Noise {
    state: u32,
}
impl Noise {
    fn new(seed: u32) -> Self {
        Self { state: seed | 1 }
    }
    #[inline]
    fn next(&mut self) -> f32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        (self.state as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// 1/f (pink) noise: octave-spaced one-pole-filtered white sources summed
/// (Voss-McCartney-style). Natural/musical fluctuations are 1/f, not white or
/// smooth -- this is what the ear reads as a *living* instrument rather than a
/// synthetic one. Output ~[-1,1], low-frequency-weighted, updated at control rate.
#[derive(Debug, Clone)]
struct Pink {
    n: Noise,
    s: [f32; 5],
    out: f32,
}
impl Pink {
    fn new(seed: u32) -> Self {
        Self { n: Noise::new(seed), s: [0.0; 5], out: 0.0 }
    }
    #[inline]
    fn next(&mut self) -> f32 {
        // Per-band one-pole lowpass coefficients (slower -> stronger / lower freq).
        const A: [f32; 5] = [0.0008, 0.004, 0.02, 0.09, 0.35];
        let mut sum = 0.0;
        for i in 0..5 {
            let w = self.n.next();
            self.s[i] += (w - self.s[i]) * A[i];
            sum += self.s[i] * (1.0 - i as f32 * 0.12);
        }
        self.out = (sum * 0.55).clamp(-1.0, 1.0);
        self.out
    }
}

#[derive(Debug, Clone)]
pub struct ArchetVoice {
    string: BowedWaveguide,
    modal: ModalString,   // Demoucron modal string (the one production model)
    // The second transverse plane of a plucked string, a fraction sharp of
    // the first: every partial of a recorded pluck is a doublet that beats.
    modal_b: ModalString,
    // Published-model pluck (Välimäki 2004): a SHAPED excitation buffer (quill
    // scrape, computed at note-on, fed into the string sample-by-sample) plus a
    // direct LF key-KNOCK that bypasses the string; the release fires a thump.
    exc_buf: Vec<f32>,
    exc_pos: usize,
    modal_fscale: f32,    // bow-force scale into the modal friction
    modal_gain: f32,      // output calibration gain for the modal bridge force
    modal_scratch: f32,   // rosin-scratch mix into the modal bridge (calibrated 0.4)
    modal_recalc: u32,    // throttles the vibrato coefficient recompute (CPU)
    modal_release_factor: f32, // per-sample modal decay on bow-off (~150 ms détaché)
    modal_pitch_gain: f32, // pitch-compensating output gain (low notes are ~10 dB too loud)
    friction: Friction,
    body: ModalBody,
    // The (inst, detune, bridge) the current `body` was built with. The modal body
    // is deterministic in these and they are CONSTANT across most notes in a passage
    // (same instrument, same desk detune), so we rebuild it only when they actually
    // change and otherwise reset its filter state in place -- avoiding a ~60-push
    // Vec heap build on EVERY note_on (x8 in ensemble fire_unison = the dense-section
    // RT spike). Bit-identical: same params -> same coefficients, reset() zeroes state.
    body_inst: usize,
    body_detune: f32,
    body_bridge: f32,
    noise: Noise,
    noise_lp: f32,
    noise_bp: Biquad, // bandpass shaping the rosin scratch (~2.8 kHz)
    grit_bp: Biquad,  // low "grit" band (~520 Hz) -- the bow grabbing the string
    // humanization (separate noise stream so it doesn't colour the bow scratch)
    hum: Noise,
    wander: f32,       // slow random drift of vibrato rate/depth
    flutter: f32,      // fast micro-pitch jitter
    vib_rate_jit: f32, // per-note vibrato-rate multiplier
    vib_depth_jit: f32,// per-note vibrato-depth multiplier
    // 1/f psychoacoustic micro-modulation (the "aliveness")
    pink_pitch: Pink,  // pitch jitter
    pink_amp: Pink,    // amplitude shimmer
    pink_bow: Pink,    // bow-pressure -> time-varying brightness (spectral flux)
    shimmer: f32,      // current 1/f amplitude-shimmer gain
    // The string a plucked note occupies, as (instrument, string), so the next
    // note on that string can end it; and whether it has been ended that way.
    on_string: Option<(usize, usize)>,
    stopped: bool,
    stop_damp: f32, // per-sample modal decay once a finger lands on the string

    pub note: Option<u8>,
    /// Monotonic note-on sequence number (set by the engine): note_off releases only
    /// the OLDEST voice of a pitch, so on overlapping REPEATED notes the previous
    /// note's off can't choke the freshly-started one (the "note_off gates all" bug
    /// class -- worst audible on the harpsichord's repeated 16ths).
    pub on_seq: u64,
    inst_idx: usize, // per-note instrument body (0 violin..3 bass), from pitch if auto_range
    vel: f32,
    sr: f32,
    voice_idx: usize,

    // bow state
    bow_vel_target: f32,
    bow_force_target: f32,
    bow_force: f32, // smoothed (the attack/release ramp)
    bow_dir: f32,   // bow direction +1/-1; flips at a détaché bow change
    force_dip: f32, // transient bow-force reduction at a bow change (ramps back to 1)
    // Bow-stroke GESTURE: the within-note arch of bow velocity (rise -> breathing
    // sustain -> shaped fall). A static rectangle is a free reed (accordion); a real
    // stroke rises, peaks and decays (Demoucron Ch.5-6), and because bow force follows,
    // loudness and brightness CO-VARY through the note -- the expression.
    stroke: f32,        // current gesture value (multiplies bow_vel/force targets)
    stroke_rise_t: f32, // per-note rise time (8-60 ms; accents fast, soft notes slow)
    stroke_fall_t: f32, // per-note fall time on note_off (50-90 ms; longer when low)
    stroke_floor: f32,  // sustain decay floor (low strings breathe more)
    stroke_tau: f32,    // sustain decay time constant (s)
    releasing: bool,
    amp_env: f32,   // amplitude attack envelope (soft bow onset, 0->1)
    note_attack: f32, // per-note attack time (velocity-dependent: accents punchier)
    freq_hz: f32,     // current sounding frequency (sizes the bow-force establishment time)
    freq_target: f32, // legato glide target (freq_hz ramps to it over ~20 ms = slur crossfade)
    vib_amt: f32,     // per-note vibrato scale (soft/short notes vibrate less)

    // vibrato
    vib_phase: f32,
    note_time: f32,

    // ensemble / string-section (research params): static F0 scatter (cents)
    // + stage azimuth + ONSET asynchrony (bows don't land together) + a slow
    // independent intonation DRIFT (players wander in/out of tune over s).
    unison_det: f32,
    unison_pan: f32,
    onset_delay: usize, // samples to wait before this player's bow engages
    release_delay: i32, // samples until this player lifts the bow (-1 = none);
                        // staggers a section's note-OFFs (bows lift apart)
    drift: f32,         // slewed slow random-walk pitch drift (raw, ~±0.003)

    // activity tracking
    energy: f32,

    // steal declick (instant-step click class): when an audibly-sounding
    // voice is retriggered (voice steal / same-note reuse), the old sound
    // fades over ~3 ms BEFORE the new note resets the modal state.
    stealing: bool,
    steal_fade: f32,
    steal_pending: Option<(u8, u8)>, // (note, vel) queued behind the fade
    /// The queued note's ensemble onset delay (set_unison runs BEFORE
    /// note_on): stashed during the fade so render() doesn't consume it
    /// as the OLD note's silence, restored when the pending note fires.
    pending_onset: usize,

    ctrl_counter: usize,
    // control-rate cached values
    bend: f32,
    bow_vel: f32,
    noise_amt: f32,
}

impl ArchetVoice {
    pub fn new(sr: f32, voice_idx: usize) -> Self {
        Self {
            string: BowedWaveguide::new(sr),
            modal: ModalString::new(sr),
            modal_b: ModalString::new(sr),
            exc_buf: Vec::new(),
            exc_pos: 0,
            modal_fscale: 1.2,
            // the modal bridge force is ~1e-3 scale (displacements ~ 1/omega^2); bring
            // it up to the engine's working level (peak ~0.3).
            modal_gain: 260.0,
            // rosin-scratch mix into the modal bridge (the calibrated value).
            modal_scratch: 0.4,
            modal_recalc: 0,
            // Decay to a tenth on bow-off, against the long natural modal ring
            // that made short notes sound plucked. Only the value a voice holds
            // until its first note-off: from then on `note_off` derives it from
            // the patch, and the time written here is the shorter of the two a
            // detache stroke wants.
            modal_release_factor: (0.1f32).powf(1.0 / (0.08 * sr)),
            modal_pitch_gain: 1.0,
            friction: Friction::new(sr),
            body: ModalBody::new(sr, 0, 1.0, 9.0),
            body_inst: 0,
            body_detune: 1.0,
            body_bridge: 9.0,
            noise: Noise::new(0x9E37_79B9 ^ ((voice_idx as u32).wrapping_mul(2654435761))),
            noise_lp: 0.0,
            noise_bp: Biquad::bandpass(sr, 2800.0, 1.1),
            grit_bp: Biquad::bandpass(sr, 520.0, 0.9),
            hum: Noise::new(0x1234_5678 ^ ((voice_idx as u32).wrapping_mul(40503))),
            wander: 0.0,
            flutter: 0.0,
            vib_rate_jit: 1.0,
            vib_depth_jit: 1.0,
            pink_pitch: Pink::new(0xA1B2 ^ (voice_idx as u32).wrapping_mul(2246822519)),
            pink_amp: Pink::new(0xC3D4 ^ (voice_idx as u32).wrapping_mul(3266489917)),
            pink_bow: Pink::new(0xE5F6 ^ (voice_idx as u32).wrapping_mul(668265263)),
            shimmer: 1.0,
            note: None,
            on_seq: 0,
            inst_idx: 0,
            vel: 0.0,
            sr,
            voice_idx,
            bow_vel_target: 0.0,
            bow_force_target: 0.0,
            bow_force: 0.0,
            bow_dir: 1.0,
            force_dip: 1.0,
            on_string: None,
            stopped: false,
            // A finger landing re-terminates the string: what rang before is no
            // longer a mode of the new length. How fast it dies under the
            // finger is not measured anywhere; this is of the order of the
            // onset ceiling Melka measured on pizzicato, and it is a choice.
            stop_damp: (0.1f32).powf(1.0 / (0.004 * sr)),
            stroke: 0.3,
            stroke_rise_t: 0.03,
            stroke_fall_t: 0.06,
            stroke_floor: 0.8,
            stroke_tau: 1.2,
            freq_hz: 220.0,
            freq_target: 220.0,
            releasing: false,
            amp_env: 0.0,
            note_attack: 0.03,
            vib_amt: 1.0,
            vib_phase: 0.0,
            unison_det: 0.0,
            unison_pan: 0.0,
            onset_delay: 0,
            release_delay: -1,
            drift: 0.0,
            note_time: 0.0,
            energy: 0.0,
            stealing: false,
            steal_fade: 1.0,
            steal_pending: None,
            pending_onset: 0,
            ctrl_counter: 0,
            bend: 1.0,
            bow_vel: 0.0,
            noise_amt: 0.0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.note.is_some() || self.bow_force > 1e-4 || self.energy > 1e-4
    }

    /// Stage azimuth for this voice (-1 L .. +1 R); the ensemble seats each
    /// unison player. 0 for ordinary notes.
    pub fn pan(&self) -> f32 { self.unison_pan }

    /// Set the unison player's STATIC F0 offset (cents) + stage pan + onset
    /// delay (samples) before a note-on (string-section scatter + bow-attack
    /// asynchrony, see engine fire_unison). Persists across note_on; reset to
    /// (0,0,0) for ordinary notes.
    pub fn set_unison(&mut self, det_cents: f32, pan: f32, onset_delay: usize) {
        self.unison_det = det_cents;
        self.unison_pan = pan;
        self.onset_delay = onset_delay;
    }

    /// Schedule this player's bow LIFT `delay` samples from now (section
    /// release asynchrony -- players don't stop together). delay 0 = release
    /// immediately (the ordinary, single-voice behaviour).
    pub fn schedule_release(&mut self, delay: i32, patch: &ArchetPatch) {
        if delay <= 0 {
            self.note_off(patch);
        } else {
            self.release_delay = delay;
        }
    }

    /// Re-seed every stochastic stream so this voice is INDEPENDENT of the
    /// same-index voice in another engine instance (string-section
    /// decorrelation -- see ArchetPatch::seed_offset). Safe to call while
    /// idle (engine setup, before any NoteOn).
    pub fn reseed(&mut self, offset: u32) {
        let m = |k: u32| -> u32 {
            (self.voice_idx as u32).wrapping_mul(k) ^ offset.wrapping_mul(2654435761).rotate_left(13)
        };
        self.noise      = Noise::new(0x9E37_79B9 ^ m(2654435761));
        self.hum        = Noise::new(0x1234_5678 ^ m(40503));
        self.pink_pitch = Pink::new(0xA1B2 ^ m(2246822519));
        self.pink_amp   = Pink::new(0xC3D4 ^ m(3266489917));
        self.pink_bow   = Pink::new(0xE5F6 ^ m(668265263));
    }

    fn freq_of(note: u8) -> f32 {
        phonix_music::midi::hz_of(note as f32)
    }
    /// Pitch-compensating output gain: the modal level falls ~1.7 dB/oct with pitch
    /// (low notes have many more modes + larger 1/omega^2 amplitudes), so the bass was
    /// ~10 dB too loud vs the top. Boost with pitch to flatten toward the reference's
    /// gentle ~-1.1 dB/oct, with extra attenuation of the very bottom octave.
    fn modal_pgain(freq: f32) -> f32 {
        // Calibrated on the MIX, not isolated notes: flattening per-note level (1.5)
        // made the aggregate treble-heavy (the melody sits high) -> -9.3 dB bass/treble
        // vs the acceptable VP-330's -3.7. 0.4 brings the mix balance back near VP-330.
        let db_per_oct = 1.0f32;
        let g = (freq / 220.0).powf(db_per_oct / 6.0205);
        // tame the sub-bass (below ~70 Hz) which sat disproportionately hot
        let sub = (freq / 70.0).min(1.0).powf(0.5);
        g * sub
    }
    /// Per-instrument modal-string parameters: (B1 base damping, B2 damping growth
    /// -> bigger = darker / faster high-partial decay, stiffness inharmonicity, beta
    /// bow position). The low instruments use a larger B2 so high partials die fast
    /// (the physical reason a cello/bass is darker than a violin -- per-partial decay,
    /// the thing a waveguide can't do).
    /// Per-note instrument body: in `auto_range` (ensemble) mode pick it from the
    /// note's PITCH (a violin can't play below G3; cellos/basses carry the low notes)
    /// so ONE engine renders a full-range composite desk; otherwise the fixed patch
    /// instrument. Bands: >=G3(55) violin | C3..F#3 viola | C2..B2 cello | <C2 bass.
    /// The string a note is played on, in first position: the highest open
    /// string at or below it. Returns that string's index, its open frequency
    /// and the instrument's sounding length, from which the sounding length of
    /// a stopped note follows. Tunings are the standard ones; lengths are
    /// Fletcher and Rossing, The Physics of Musical Instruments, Springer 1991,
    /// table 10.1 (after Hutchins 1980), at the middle of each range.
    /// Measured decay of the violin's open strings, plucked: the University of
    /// Iowa Electronic Music Studios instrument samples, anechoic, mf, one
    /// exponential fitted per partial between three and thirty dB down, kept
    /// only where the fit is clean and the band holds no partial of another
    /// open string (the fifths coincide: three G is two D, three D two A,
    /// three A two E). Pairs of (hertz, seconds to sixty dB down) per string.
    /// They carry what no smooth law does: the fundamental rings longest,
    /// neighbouring partials differ several-fold, and the upper partials of
    /// the A and E are gone within a third of a second. A stopped note on the
    /// string reads the same curve at its own partial frequencies.
    ///
    /// The other three instruments come from the same collection, measured
    /// the same way, with the reserves their recordings impose: the viola's
    /// files are noise-gated, so every figure extrapolates a short window of
    /// decay before the gate; the cello's and bass's floor sits only some
    /// thirty dB under the notes; and on the bass every partial of every
    /// string lies within twenty hertz of a harmonic of the low E, so the
    /// coincidence rule cannot be met there. A partial is kept when its fit
    /// is clean and it either sits clear of every other open string's
    /// harmonics or is shorter than the one it sits on, which a blend cannot
    /// make it; where a report named the string's own early slope under a
    /// longer sympathetic partial, that slope is the value.
    pub(crate) fn measured_pizz_t60(body_index: usize, string: usize) -> Option<&'static [(f32, f32)]> {
        const VIOLIN: [&[(f32, f32)]; 4] = [
            &[
                (194.7, 5.51), (388.0, 2.15), (780.1, 2.00), (975.4, 2.39),
                (1171.2, 0.98), (1363.0, 1.41), (1558.2, 1.49),
            ],
            &[(291.0, 3.86), (581.4, 2.47), (871.4, 1.29), (1163.7, 1.58)],
            &[(440.2, 3.31), (1755.5, 0.22), (2641.0, 0.27)],
            &[
                (657.7, 2.63), (1316.6, 1.02), (1969.2, 0.96), (2633.1, 0.70),
                (3291.3, 1.22), (3947.5, 0.26),
            ],
        ];
        const VIOLA: [&[(f32, f32)]; 4] = [
            &[
                (130.2, 12.0), (260.0, 13.1), (391.8, 2.90), (522.7, 2.63),
                (652.1, 1.19), (784.6, 1.65), (913.7, 1.27), (1049.6, 1.44),
            ],
            &[(194.8, 5.16), (978.1, 1.37), (1177.3, 1.19), (1368.0, 1.67)],
            &[(292.5, 3.48), (879.0, 2.01), (1466.6, 1.19), (1763.7, 0.71), (2053.7, 0.48)],
            &[(438.0, 1.22), (881.5, 3.19), (1317.7, 1.23), (2194.9, 0.66), (3077.4, 0.81)],
        ];
        const CELLO: [&[(f32, f32)]; 4] = [
            &[(65.8, 7.08), (130.6, 5.13), (261.9, 5.54), (328.6, 3.25), (460.2, 2.86), (526.6, 1.68)],
            &[
                (98.1, 9.63), (490.2, 3.48), (588.5, 3.12), (687.4, 3.36),
                (786.3, 0.96), (883.4, 1.20), (982.1, 1.37),
            ],
            &[(145.4, 8.52), (292.4, 1.91), (439.9, 2.24), (733.0, 2.67), (1027.7, 1.65), (1466.9, 1.44)],
            &[(219.2, 2.86), (438.9, 3.75), (658.7, 1.41), (1096.8, 1.88), (1537.8, 2.26), (1755.5, 0.97)],
        ];
        const BASS: [&[(f32, f32)]; 4] = [
            &[(41.0, 10.8), (81.6, 9.00), (124.7, 3.06)],
            &[(54.5, 6.68), (110.4, 1.23), (220.6, 2.73), (387.7, 1.30), (444.6, 1.09)],
            &[
                (72.7, 11.4), (145.8, 6.49), (219.0, 4.60), (292.0, 7.95), (365.3, 3.98),
                (438.0, 3.93), (584.3, 2.69), (661.4, 0.94), (730.8, 1.52),
            ],
            &[
                (96.5, 2.19), (195.3, 2.0), (293.3, 4.15), (390.9, 4.79), (489.1, 5.53),
                (685.8, 3.33), (784.5, 1.24), (982.4, 1.37),
            ],
        ];
        let tables = match body_index {
            0 => &VIOLIN,
            1 => &VIOLA,
            2 => &CELLO,
            _ => &BASS,
        };
        Some(tables[string.min(3)])
    }

    /// Decay time at a frequency, from a measured table: straight in log
    /// frequency against log time between the points, held at the end values
    /// beyond them.
    fn t60_from_table(table: &[(f32, f32)], hz: f32) -> f32 {
        let (f_lo, t_lo) = table[0];
        let (f_hi, t_hi) = table[table.len() - 1];
        if hz <= f_lo {
            return t_lo;
        }
        if hz >= f_hi {
            return t_hi;
        }
        for w in table.windows(2) {
            let ((f1, t1), (f2, t2)) = (w[0], w[1]);
            if hz >= f1 && hz <= f2 {
                let x = (hz / f1).ln() / (f2 / f1).ln();
                return (t1.ln() + x * (t2.ln() - t1.ln())).exp();
            }
        }
        t_hi
    }

    pub(crate) fn string_for(body_index: usize, note: u8) -> (usize, f32, f32) {
        let (opens, length_m): (&[u8; 4], f32) = match body_index {
            0 => (&[55, 62, 69, 76], 0.327), // violin: G3 D4 A4 E5
            1 => (&[48, 55, 62, 69], 0.375), // viola: C3 G3 D4 A4
            2 => (&[36, 43, 50, 57], 0.685), // cello: C2 G2 D3 A3
            _ => (&[28, 33, 38, 43], 1.105), // double bass: E1 A1 D2 G2
        };
        let string = opens.iter().rposition(|&o| o <= note).unwrap_or(0);
        (string, Self::freq_of(opens[string]), length_m)
    }
    pub(crate) fn inst_for(note: u8, patch: &ArchetPatch) -> usize {
        if patch.auto_range {
            match note {
                n if n >= 55 => 0,
                48..=54 => 1,
                36..=47 => 2,
                _ => 3,
            }
        } else {
            patch.instrument.body_index()
        }
    }
    /// Per-instrument MIX level: the storm is the upper strings carrying the rapid
    /// figuration over a SUPPORTING bass. Summing all desks near-equal buried the
    /// violins under the bass. Push violins forward, bass well back. ARCHET_LOWMIX
    /// (env) scales the low strings so the bass weight is one knob.
    fn inst_level(idx: usize) -> f32 {
        let low = 1.0f32;
        match idx {
            0 => 1.40,        // violin: figuration/melody well forward
            1 => 0.70,        // viola
            2 => 0.34 * low,  // cello
            _ => 0.18 * low,  // contrabass: support, well back
        }
    }
    fn modal_params(body_index: usize, freq: f32) -> (f32, f32, f32, f32) {
        // Clean+bright regime: low B2 (bright Helmholtz corner) is CLEAN because the
        // stiffness is realistic (~1-5e-5) -- the old ~1e-3 stiffness scattered the high
        // modes off the harmonic grid so they read as noise. (b1, b2 per-partial damping,
        // stiff inharmonicity, beta bow position.)
        let (b1, b2, stiff, beta) = match body_index {
            0 => (1.6, 0.045, 2.0e-5, 0.075), // violin
            1 => (1.4, 0.055, 3.0e-5, 0.080), // viola
            2 => (1.2, 0.060, 4.0e-5, 0.082), // cello
            _ => (1.0, 0.075, 6.0e-5, 0.082), // double bass
        };
        // High notes have FEW modes and ring chaotically at fixed B2 -> scale per-partial
        // damping up with pitch so the highs stay clean (mids/lows ~unchanged, still bright).
        let b2 = b2 * (1.0 + (freq / 750.0).powi(2));
        let b1m = 1.0f32;
        let b2m = 1.0f32;
        (b1 * b1m, b2 * b2m, stiff, beta)
    }

    /// The string's own loss coefficients, for the plucked articulation:
    /// friction (internal) and air, with bending shared. Woodhouse, Plucked
    /// guitar transients, Acta Acustica 90 (2004), eq. 8: the loss factor of
    /// mode n is
    ///
    ///   eta_n = [eta_F + eta_A / w_n + s n^2 eta_B] / [1 + s n^2]
    ///
    /// in this crate's normalisation, where `s` is the stiffness already used
    /// for the mode frequencies. The FORM is published; the values are fitted,
    /// measured violin coefficients (Pickering, Catgut Acoust. Soc. J. 44,
    /// 1985) not being freely available.
    ///
    /// Violin and viola are fitted to the one decay SHAPE measured on this
    /// family -- decay time inversely proportional to frequency (Powell,
    /// Acoustic Analysis of the Viola, NSF REU, UIUC 2012, fitted exponent
    /// -1.006) -- each anchored at its own lowest open string, so the overall
    /// character is untouched and only the pitch dependence moves. The air
    /// term's damping RATE is eta_A / 2, the same at every pitch, so an
    /// oversized eta_A flattens the whole law: that is what kept the middle
    /// register ringing on like a plucked zither instead of a violin.
    ///
    /// Cello and bass keep the older fit. The same procedure reaches the shape
    /// for them too, but only by driving their air term to nearly nothing,
    /// where the loss tables in the paper above hold the air coefficient
    /// roughly constant across strings and let friction vary widely. Their
    /// decay stays nearly pitch independent, which is a defect still open.
    fn string_losses(body_index: usize) -> (f32, f32) {
        match body_index {
            0 => (3.364e-3, 2.486), // violin
            1 => (6.150e-3, 0.774), // viola
            2 => (8.169e-4, 5.13),  // cello
            _ => (9.752e-4, 5.18),  // double bass
        }
    }
    fn freq_tuned(note: u8, patch: &ArchetPatch) -> f32 {
        Self::freq_of(note) * 2f32.powf(patch.tune_cents / 1200.0)
    }

    /// Per-note EXPRESSION (shared by fresh bows and legato retunes): velocity
    /// and pitch drive brightness (string damping + bow position), loudness, the
    /// per-note attack, and vibrato amount -- so notes shade musically.
    fn apply_expression(&mut self, note: u8, v: f32, patch: &ArchetPatch) {
        // Pitch-dependent brightness (octaves above D4=62; Schelleng) + velocity
        // brightness (pp dark/flautando, ff brilliant).
        let oct = (note as f32 - 62.0) / 12.0;
        let bright = (1.0 + oct * 0.45).clamp(0.6, 2.6);
        let vbr = (v - 0.55).clamp(-0.55, 0.45);
        self.string.bow_pos = (patch.bow_pos - oct * 0.011 - vbr * 0.028).clamp(0.055, 0.20);
        self.string.loss = (patch.loss - oct * 0.03 - vbr * 0.16).clamp(0.05, 0.85);
        self.string.tor_ratio = patch.tor_ratio.clamp(2.0, 8.0);
        self.string.tor_couple = patch.tor_couple.clamp(0.0, 0.5);
        self.string.tor_inject = patch.tor_inject.clamp(0.0, 0.5);

        // The second (vertical) polarization is a string detuned ~0.25% and summed
        // at 0.22 -- on a violin that warm two-plane beat is right, but at 40-80 Hz
        // it is audibly a SECOND DETUNED OSCILLATOR (a supersaw), which is exactly
        // why the bass reads as a synth: it desyncs the waveform (measured periodicity
        // 0.91 vs a real bass's 0.96) and triples the spectral flux. Collapse it in
        // the low register so the bass is one clean, periodic bowed string; keep the
        // full two-plane warmth on the violin (index 0 unchanged).
        let (vmix, vdet) = match self.inst_idx {
            0 => (0.22, 0.0025),  // violin: full two-plane warmth
            1 => (0.16, 0.0020),  // viola
            2 => (0.09, 0.0014),  // cello
            _ => (0.04, 0.0008),  // double bass: essentially single, clean string
        };
        self.string.vert_mix = vmix;
        self.string.vert_detune = vdet;

        let dyn_ = 0.30 + 0.70 * v * v; // pp .. ff
        let r1 = self.hum.next();
        let r2 = self.hum.next();
        let r3 = self.hum.next();
        let r4 = self.hum.next();
        let r5 = self.hum.next();
        let r6 = self.hum.next();
        // ENSEMBLE: EVERY player differs ("tout devrait être différent par
        // violon") -- not just pitch, but attack speed, release speed, bow
        // force/loudness and vibrato. The per-voice seeds already make these
        // independent; `js` widens the spread so a section reads as many
        // distinct players, not clones. (Meyer: a section's defining traits
        // are desynchronized vibrato + spread attacks/releases + per-player
        // dynamics.) A solo keeps the tight, expressive spread.
        let ens = patch.ensemble >= 1.5;
        let js = if ens { 2.4 } else { 1.0 }; // per-voice variation scale
        self.bow_force_target = patch.bow_force * dyn_ * bright * (1.0 + r3 * 0.07 * js);
        self.bow_vel_target = patch.bow_vel * (0.5 + 0.5 * v) * bright.sqrt() * (1.0 + r4 * 0.08 * js);
        self.note_attack = (patch.attack * (1.6 - 0.8 * v) * (1.0 + r1 * 0.15 * js)).clamp(0.010, 0.14);
        // desynchronized, continuous, deeper vibrato for the section
        if ens {
            self.vib_amt = (0.70 + 0.30 * v) * (1.0 + r2 * 0.15);
            self.vib_rate_jit = 1.0 + r1 * 0.22;          // ±22% rate (~4.3-6.7 Hz)
            self.vib_depth_jit = (1.0 + r2 * 0.40) * 1.4; // wider + deeper extent
        } else {
            self.vib_amt = (0.35 + 0.8 * v) * (1.0 + r2 * 0.15);
            self.vib_rate_jit = 1.0 + r1 * 0.10;
            self.vib_depth_jit = 1.0 + r2 * 0.20;
        }
        // Per-note GESTURE shape: attack rise + release fall times jittered
        // PER VOICE (each violin bows in/out at its own speed), widened for
        // the section so no two strokes share a shape.
        let lowi = self.inst_idx >= 2;
        self.stroke_rise_t = ((0.055 - 0.047 * v) * (1.0 + r5 * 0.25 * js)).clamp(0.008, 0.090);
        // The release control belongs HERE, not only on the ring-out: what a
        // listener hears as the end of a bowed note is the gesture's fall, the
        // bow decelerating off the string. Amplitude follows the stroke, whose
        // time constant is a third of this, so the control spans a real range.
        // Low strings take longer to let go, and the per-note spread stays.
        let fall = patch.release.clamp(0.02, 0.9) * if lowi { 1.55 } else { 1.0 };
        self.stroke_fall_t = (fall * (1.0 + r6 * 0.20 * js)).max(0.03);
        self.stroke_floor = (if lowi { 0.65 } else { 0.80 }) * (1.0 + r5 * 0.05);
        self.stroke_tau = (if lowi { 0.8 } else { 1.2 }) * (1.0 + r6 * 0.20);
    }

    pub fn note_on(&mut self, note: u8, vel: u8, patch: &ArchetPatch) {
        // STEAL DECLICK: a fresh attack slams amp_env to 0 and resets the
        // modal string — an instant step from whatever this voice was
        // radiating (the audible click on every voice steal / same-note
        // retrigger). If the voice is still audibly sounding, queue the
        // note behind a ~3 ms fade-out instead (see process()).
        if self.is_active() && (self.energy > 1e-3 || self.stealing) {
            if !self.stealing {
                self.stealing = true;
                self.steal_fade = 1.0;
            }
            self.steal_pending = Some((note, vel));
            // set_unison ran before this note_on: park the NEW note's
            // onset delay so render() keeps playing the OLD sound during
            // the fade instead of muting it behind the fresh delay.
            self.pending_onset = self.onset_delay;
            self.onset_delay = 0;
            // Track the incoming pitch immediately so NoteOff pairing
            // works even while the old sound is still fading.
            self.note = Some(note);
            self.releasing = false;
            self.release_delay = -1;
            return;
        }
        self.note_on_now(note, vel, patch);
    }

    fn note_on_now(&mut self, note: u8, vel: u8, patch: &ArchetPatch) {
        let v = (vel as f32 / 127.0).clamp(0.0, 1.0);
        self.vel = v;
        self.note = Some(note);
        self.release_delay = -1; // fresh/retriggered note: cancel any pending lift
        self.inst_idx = Self::inst_for(note, patch);
        self.releasing = false;
        self.amp_env = 0.0; // start silent -> the bow attack ramps the loudness in
        self.note_time = 0.0;
        // Random vibrato phase per note: a real section's players vibrate INDEPENDENTLY
        // (smears the partials -> rich ensemble); starting every voice at phase 0
        // synchronizes them -> a chorused reed bank = the "accordion" sum.
        self.vib_phase = self.hum.next() * 0.5 + 0.5;
        self.energy = 0.0;

        // Per-voice + per-desk body detune so a section is many distinct
        // instruments (no body combing at unisons). tune_cents differs per desk.
        let detune = 1.0 + ((self.voice_idx as f32 * 0.013).sin()) * 0.012 + patch.tune_cents * 0.0007;
        // Rebuild the modal body ONLY when its inputs change (rare); otherwise reset
        // the cached body in place -- same params => identical coefficients, so this is
        // bit-identical to an unconditional rebuild but avoids the per-note Vec build.
        if self.inst_idx != self.body_inst
            || (detune - self.body_detune).abs() > 1e-9
            || (patch.bridge_hill_db - self.body_bridge).abs() > 1e-9
        {
            self.body = ModalBody::new(self.sr, self.inst_idx, detune, patch.bridge_hill_db);
            self.body_inst = self.inst_idx;
            self.body_detune = detune;
            self.body_bridge = patch.bridge_hill_db;
        } else {
            self.body.reset();
        }

        self.apply_expression(note, v, patch);
        self.bow_dir = 1.0;       // fresh stroke: bow drawn in the reference direction
        self.force_dip = 1.0;
        // unison_det: this player's STATIC section detune (cents) on top of
        // the patch/desk tuning -- the Ternström frequency scatter.
        self.freq_hz = Self::freq_tuned(note, patch) * 2f32.powf(self.unison_det / 1200.0);
        self.freq_target = self.freq_hz; // no glide on a fresh attack
        self.modal_pitch_gain = Self::modal_pgain(self.freq_hz) * Self::inst_level(self.inst_idx);

        // PLUCKED articulation (pizzicato): a fingertip excitation then a free
        // modal ring-down -- the bow machinery is bypassed entirely (see
        // process()).
        if patch.pluck {
            // PUBLISHED-MODEL pluck (Välimäki/Penttinen EURASIP 2004, complete
            // architecture -- no fragments):
            // (a) pluck point: the hand sits at the end of the fingerboard
            // whatever the note, about a fifth of the OPEN length from the
            // bridge, and stopping shortens the length that sounds. So the
            // point is a fixed spot, and the fraction of the sounding length
            // it falls at grows with the pitch played on that string; the
            // fraction, not the spot, is what sets the comb.
            // A point release at an exact fraction 1/d of the length puts the
            // pluck on a node of modes d, 2d, 3d and gives them zero amplitude,
            // so a fifth destroys the 5th, 10th and 15th harmonics and the note
            // sounds hollow. Measured spectra never do that: a partial with a
            // node at the plucking position is strongly attenuated, not absent
            // (Traube, An interdisciplinary study of the timbre of the classical
            // guitar, McGill 2004, 4.5.1 -- the whole plucking-point estimation
            // literature works by fitting the amplitudes left in the valleys).
            // What is measured is that the position MOVES: between the fingers
            // of one player it spans up to one percent of the string length, and
            // its variance grows with register (Chadefaux, Le Carrou and Fabre,
            // Experimentally-based description of harp plucking, JASA 2012). So
            // draw it per note over that span, which leaves the comb in place
            // for each note and moves it from note to note, as a hand does. No
            // published measurement pins the spot for a violin, so it stays at
            // a fifth of the open length. The spread is half a percent each
            // way, and the fraction folds about the middle, where the comb is
            // symmetric.
            let (string, f_open, l_open) = Self::string_for(self.inst_idx, note);
            self.on_string = Some((self.inst_idx, string));
            self.stopped = false;
            let spot = 0.20 * Self::freq_of(note) / f_open;
            let spot = if spot > 0.5 { 1.0 - spot } else { spot };
            let p = (spot + self.hum.next() * 0.005).clamp(0.02, 0.5);
            // (b) the same string the bow uses, with the same stiffness law.
            let (_, _, stiff, _) = Self::modal_params(self.inst_idx, self.freq_hz);
            // (c) per-partial decay from the string's losses rather than a
            // fitted curve: friction, air and bending, combined by Woodhouse's
            // eq. 8 (see `string_losses`). The curve this replaces floored
            // every partial at 100 ms, so a top-string pizzicato kept its high
            // modes alive a tenth of a second at every pitch; the losses let
            // mode 40 die in 16 ms up there, which is what a plucked string
            // does. Its cosine ripple is gone with it: real strings ripple
            // because the two polarisations beat, and this model has one
            // polarisation, so the ripple was decoration.
            let (eta_f, eta_a) = Self::string_losses(self.inst_idx);
            const ETA_B: f32 = 2.0e-2;
            let f0 = self.freq_hz;
            // (d) and the body drains the string, which is the channel that
            // makes a plucked violin a short sound. Cremer's termination,
            // fitted to a violin A string and given by Woodhouse (On the
            // playability of violins I, Acustica 78, eq. 14 and 21): the
            // bridge presents Y = i w Y0 / (MU + i w LAMBDA), so the share of
            // its own admittance the string sees taken at mode n is
            // w^2 LAMBDA / (MU^2 + w^2 LAMBDA^2), lost once per round trip and
            // f0 round trips a second. Without it the string keeps everything
            // but its internal losses and rings like a harp.
            const LAMBDA: f32 = 39.0;
            const MU: f32 = 4.0e5;
            // The measured curve of the string this note is on, where one
            // exists; the fitted law otherwise.
            let measured = Self::measured_pizz_t60(self.inst_idx, string);
            let t60law = move |k: usize| -> f32 {
                let kf = k as f32;
                let sk = stiff * kf * kf;
                let wn = std::f32::consts::TAU * f0 * kf * (1.0 + sk).sqrt();
                if let Some(table) = measured {
                    return Self::t60_from_table(table, wn / std::f32::consts::TAU);
                }
                let eta = (eta_f + eta_a / wn + sk * ETA_B) / (1.0 + sk);
                let wl = wn * LAMBDA;
                let body = 2.0 * f0 * (wn * wn * LAMBDA / (MU * MU + wl * wl));
                (6.9078 / (eta * wn * 0.5 + body)).max(0.005)
            };
            self.modal.noise_amt = 0.0;
            self.modal.set_voice_t60(self.freq_hz, stiff, p, &t60law);
            self.modal.reset();
            // (e) a string vibrates in two transverse planes with slightly
            // different effective lengths, so every partial is a doublet and
            // beats. On the anechoic Iowa recordings of the open violin
            // strings the reliable doublets sit a fraction of a percent
            // apart with the second line some decibels under the first; the
            // two figures here are the medians of those measurements, the
            // detune of the second plane and its level. The two planes share
            // the loss table, since their losses were not measured apart.
            const PLANE_DETUNE: f32 = 0.003;
            const PLANE_LEVEL: f32 = 0.355;
            self.modal_b.noise_amt = 0.0;
            self.modal_b.set_voice_t60(self.freq_hz * (1.0 + PLANE_DETUNE), stiff, p, &t60law);
            self.modal_b.reset();
            // (d) the pluck is a RELEASE, not a blow. The finger pulls the
            // string aside and lets go, so the modes start at a displacement
            // that falls as 1/n^2 and at zero velocity. Harder plucking pulls
            // the string FURTHER, so the velocity sets the displacement rather
            // than a force. The lowpass is the fingertip's own compliance,
            // rounding the corner of the triangle.
            let h = 5200.0f32 * (0.35 + 0.65 * self.vel);
            // The fingertip covers a span of the string, not a point, and that
            // span low-passes the initial shape at a corner of the wave speed
            // over twice the span (Chadefaux, Le Carrou and Fabre, JASA 2012,
            // eq. 10), the width of a finger in contact with a string being
            // measured there at about two centimetres. The wave speed is twice
            // the OPEN length times the open-string frequency, a property of
            // the string and not of the note, so the corner is fixed in hertz
            // on each string: a fixed harmonic rank on the open string, and a
            // lower rank the higher a note is stopped, which is why a high
            // stopped pizzicato is rounder than an open one. It differs between
            // instruments because one finger spans less of a longer string.
            const FINGER_M: f32 = 0.020;
            let plp = l_open * f_open / FINGER_M;
            // 116, where the force path used 0.6. A release is not quieter by
            // mistake: the bridge force sums the modes weighted by k, so the
            // old impulse drew most of its loudness from upper partials it had
            // no business exciting, and matching the fundamental alone costs
            // ~24 dB. This gain is a voicing constant, measured rather than
            // derived: it puts the note back at the peak the bank was voiced
            // against (0.0565 on the repeat profile at D4, velocity 100).
            let amp = h * 116.0 * (1.0 + self.hum.next() * 0.05);
            self.modal.release(amp, plp, self.freq_hz);
            self.modal_b.release(amp * PLANE_LEVEL, plp, self.freq_hz * (1.0 + PLANE_DETUNE));
            // (e) the release: the string leaving the fingertip makes a brief
            // scrape, an order quieter and shorter than a quill's, fed into the
            // string so it is pitch-correlated rather than added noise.
            let n_exc = (0.02 * self.sr) as usize;
            self.exc_buf.clear();
            self.exc_buf.reserve(n_exc);
            let mut lp = 0.0f32;
            for i in 0..n_exc {
                let t = i as f32 / self.sr;
                let w = self.noise.next();
                lp += (w - lp) * 0.12;
                self.exc_buf.push(lp * (-(t / 0.004)).exp() * h * 0.004);
            }
            self.exc_pos = 0;
            self.bow_force = 0.0;
            self.bow_vel = 0.0;
            self.amp_env = 1.0;
            return;
        }

        self.string.note_on(self.freq_hz); // fresh bow stroke: prime + attack
        // PERFECT START (Guettler 2002; Woodhouse Euphonics 9.5): the bow presses at
        // full force from the first sample so the primed Helmholtz corner is maintained
        // (no synth brightness sweep). The STROKE GESTURE starts at 0.3 (the bow never
        // catches at full speed) and rises over stroke_rise_t -- since the modal force
        // follows |bow_vel|, the Schelleng ratio stays playable while loudness and
        // brightness arch together through the note.
        self.stroke = 0.3;
        // force consistent with the gesture (stroke^1.3) -- full force at a 0.3-speed
        // catch overshoots the first Helmholtz capture into an onset SPIKE that beats
        // the arch peak (percussive start). The corner still forms within ~1 period.
        self.bow_force = self.bow_force_target * 0.21;
        self.bow_vel = self.bow_vel_target * self.stroke;

        {
            let (b1, b2, stiff, beta) = Self::modal_params(self.inst_idx, self.freq_hz);
            // Bow noise is a wheezy/reedy "accordion" buzz when summed across the low
            // ensemble -> keep a touch on the violin, near-zero on the low strings.
            self.modal.noise_amt = match self.inst_idx { 0 => 0.12, 1 => 0.05, _ => 0.02 };
            self.modal.set_voice(self.freq_hz, b1, b2, stiff, beta);
            self.modal.reset(); // fresh bow stroke: silent string, perfect-start bow catches it
        }

        self.friction.mode = patch.friction.into();
        self.friction.slope = patch.slope.max(0.5);
        self.friction.reset();
        self.wander = 0.0;
        self.flutter = 0.0;
    }

    /// Same-string LEGATO slur: the bow keeps going in the SAME direction, only the
    /// finger changes pitch. The pitch must NOT jump (abruptly retuning the string is
    /// Jaffe & Smith's "spurious pluck" = the weird slur); instead GLIDE freq_hz to the
    /// new pitch over ~20 ms (a 15-30 ms crossfade is the smooth-slur window). amp_env
    /// and bow direction continue -> connected, non-choppy.
    pub fn note_on_legato(&mut self, note: u8, vel: u8, patch: &ArchetPatch) {
        let v = (vel as f32 / 127.0).clamp(0.0, 1.0);
        self.vel = v;
        self.note = Some(note);
        self.release_delay = -1; // fresh/retriggered note: cancel any pending lift
        self.inst_idx = Self::inst_for(note, patch);
        self.releasing = false;
        let keep_dir = self.bow_dir; // same bow stroke
        self.apply_expression(note, v, patch);
        self.bow_dir = keep_dir;
        self.bow_vel_target *= self.bow_dir;
        self.freq_target = Self::freq_tuned(note, patch); // glide target (ramped in process)
        self.modal_pitch_gain = Self::modal_pgain(self.freq_target) * Self::inst_level(self.inst_idx);
        self.string.retune(self.freq_target); // waveguide path: connected retune
    }

    /// DÉTACHÉ bow change: the bow does NOT lift (lifting -> free decay -> a string of
    /// PLUCKS). It stays in contact and REVERSES direction; bow force dips briefly at
    /// the velocity zero-crossing then snaps back, so Helmholtz motion transfers
    /// (reversed) instead of decaying (Demoucron Ch.6). amp_env continues (no re-attack).
    pub fn bow_change(&mut self, note: u8, vel: u8, patch: &ArchetPatch) {
        let v = (vel as f32 / 127.0).clamp(0.0, 1.0);
        self.vel = v;
        self.note = Some(note);
        self.release_delay = -1; // fresh/retriggered note: cancel any pending lift
        self.inst_idx = Self::inst_for(note, patch);
        self.releasing = false;
        self.apply_expression(note, v, patch);
        self.bow_dir = -self.bow_dir;            // reverse the bow
        self.bow_vel_target *= self.bow_dir;     // bow_vel ramps from old sign THROUGH zero
        self.force_dip = 0.18;                   // force drops at the change, then ramps back to 1
        self.note_time = 0.0;                    // re-fire the bow-grip noise burst (détaché attack)
        self.freq_hz = Self::freq_tuned(note, patch); // détaché: clean re-pitch (separate note)
        self.freq_target = self.freq_hz;
        self.modal_pitch_gain = Self::modal_pgain(self.freq_hz) * Self::inst_level(self.inst_idx);
        self.string.retune(self.freq_hz);
        {
            let (b1, b2, stiff, beta) = Self::modal_params(self.inst_idx, self.freq_hz);
            self.modal.set_voice(self.freq_hz, b1, b2, stiff, beta);
            self.modal.soften(0.5); // the reversal re-forms the corner; shed stale energy
        }
    }

    pub fn note_off(&mut self, patch: &ArchetPatch) {
        // A note released while its steal-fade is still running never
        // started sounding — cancel the pending attack (the fade keeps
        // running to zero, then the voice silences; see process()).
        self.steal_pending = None;
        // The RELEASE control is in seconds and a bow's release is a per-sample
        // modal decay, so derive one from the other here. It was frozen at
        // construction, which no automation and no preset could ever reach:
        // the control was shown, stored and automated, and moved nothing.
        // A plucked string is untouched, `process` returning on that path
        // before this factor is ever applied -- a finger leaving a string
        // damps nothing.
        let secs = patch.release.clamp(0.01, 1.0);
        self.modal_release_factor = (0.1f32).powf(1.0 / (secs * self.sr));
        self.releasing = true;
        self.note = None;
    }

    /// The string a plucked note occupies, or none for a bowed one.
    pub fn on_string(&self) -> Option<(usize, usize)> {
        self.on_string
    }

    /// A finger has landed on this voice's string for another note: end it.
    pub fn stop_string(&mut self) {
        self.stopped = true;
    }

    /// Whether a finger landing on its string has ended this plucked note.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    pub fn is_releasing(&self) -> bool {
        self.releasing
    }

    pub fn level(&self) -> f32 {
        self.energy
    }

    /// Render one sample (mono). The engine handles output level + stereo.
    ///
    /// Steal declick wrapper: while `stealing`, the OLD note keeps
    /// rendering under a ~3 ms linear fade; when the fade completes the
    /// queued note fires (fresh attack from silence — the modal reset is
    /// inaudible at gain 0). Decrement-then-test per the countdown-
    /// overshoot memory so the zero cross always fires.
    #[inline]
    pub fn process(&mut self, patch: &ArchetPatch) -> f32 {
        if self.stealing {
            self.steal_fade -= 1.0 / (0.003 * self.sr);
            if self.steal_fade <= 0.0 {
                self.stealing = false;
                self.steal_fade = 1.0;
                if let Some((n, v)) = self.steal_pending.take() {
                    self.onset_delay = self.pending_onset;
                    self.pending_onset = 0;
                    self.note_on_now(n, v, patch);
                    return self.render(patch);
                }
                // Cancelled steal (note_off arrived mid-fade): the output
                // just faded to zero, so silencing the residual state now
                // is click-free.
                self.energy = 0.0;
                self.bow_force = 0.0;
                return 0.0;
            }
            return self.render(patch) * self.steal_fade;
        }
        self.render(patch)
    }

    #[inline]
    fn render(&mut self, patch: &ArchetPatch) -> f32 {
        if !self.is_active() {
            return 0.0;
        }

        // ONSET ASYNCHRONY (ensemble): this player's bow lands a few ms late.
        // Silent + frozen (note_time doesn't advance) until the delay elapses,
        // so the section's attack is spread (~tens of ms), not one coherent
        // hit that fuses into a single fat voice.
        if self.onset_delay > 0 {
            self.onset_delay -= 1;
            return 0.0;
        }
        // staggered bow lift (section release asynchrony)
        if self.release_delay >= 0 {
            if self.release_delay == 0 {
                self.release_delay = -1;
                self.note_off(patch);
            } else {
                self.release_delay -= 1;
            }
        }

        // PLUCKED path: a pizzicato. The string rings down freely and the
        // body radiates it, which is the same corpus the bow drives -- a
        // plucked violin is a violin. Nothing damps it on note-off: a finger
        // leaves the string and the note decays by its own losses, where a
        // harpsichord drops a damper on key release.
        if patch.pluck {
            // Nothing damps a plucked note on key release; only a finger
            // landing on the same string ends it.
            let damp = if self.stopped { self.stop_damp } else { 1.0 };
            self.modal.release_damp = damp;
            self.modal_b.release_damp = damp;
            // the finger's release scrape, fed into the string
            if self.exc_pos < self.exc_buf.len() {
                let e = self.exc_buf[self.exc_pos];
                self.exc_pos += 1;
                self.modal.excite(e);
            }
            let bridge = (self.modal.process(0.0, 0.0) + self.modal_b.process(0.0, 0.0))
                * self.modal_gain
                * self.modal_pitch_gain;
            let out = self.body.process(bridge);
            self.energy += (out.abs() - self.energy) * 0.001;
            // output_level is applied ONCE, by the engine's voice-sum norm
            // (engine.rs `norm`); applying it here too squared the OUT knob.
            return out * self.amp_env;
        }

        // Register-scaled "aliveness": the 1/f micro-modulation (flux/jitter/
        // shimmer) and rosin noise are tuned for the VIOLIN's sparkle, but a real
        // bowed BASS/CELLO sustains far steadier (measured: a real contrabass note
        // has ~half the amplitude wobble, ~1/4 the spectral flux and ~16 dB less
        // inter-harmonic noise than the violin-tuned model). Left unchecked, that
        // excess wobble -- multiplied across a detuned 6-desk section -- reads as a
        // detuned synth PAD in the low end, not a bowed bass. So damp the modulation
        // by register: violin full, bass minimal. Violin (index 0) is unchanged.
        let alive = match self.inst_idx {
            0 => 1.0,  // violin: full sparkle
            1 => 0.70, // viola
            2 => 0.48, // cello
            _ => 0.36, // double bass: steady
        };

        // --- control rate: vibrato, bow ramps, noise amount ---
        if self.ctrl_counter == 0 {
            let dt = CTRL_INTERVAL as f32 / self.sr;
            self.note_time += dt;

            // 1/f micro-modulation (psychoacoustic aliveness): a constantly
            // evolving bow pressure -> time-varying brightness (spectral flux),
            // the single biggest cue separating a living instrument from a static
            // synth tone.
            let pbow = self.pink_bow.next();
            let bow_flux = 1.0 + pbow * 0.14 * alive; // +/-14% slow bow-pressure drift (register-scaled)

            // Bow force/velocity ramp. CRITICAL (acoustics, not a tweak): in a real
            // bowed string the spectral brightness is set by the bow FORCE via the
            // Helmholtz corner-sharpening<->loss balance (Schoonderwaldt 2009: centroid
            // ~ F^(1/3)), and it reaches steady sharpness within ~1-2 string periods of
            // the first slip -- then it stays FLAT or rounds slightly (Woodhouse,
            // Euphonics 9.2/9.5; Guettler 2002). It does NOT ramp up over the whole
            // onset. Ramping bow force over the full `note_attack` made the corner
            // sharpen as force accumulated -> a 400->2000 Hz brightness SWEEP, the
            // textbook synth filter-envelope cue. So establish force/velocity FAST
            // (~1 period, the string is already primed near the Helmholtz corner) so
            // brightness is steady from the first cycle; the LOUDNESS still fades in
            // separately via amp_env below. This decouples timbre (fast, force-driven)
            // from loudness (gradual) -- exactly the real onset.
            // ----- bow-stroke GESTURE (the within-note arch; the expression) -----
            // RISE 0.3 -> 1.0 over stroke_rise_t (smooth sine quarter), then a BREATHING
            // sustain (exponential decay toward stroke_floor; low strings breathe more --
            // a real stroke loses energy as the bow travels, which kills the static
            // bass drone) with a slow SWELL on long notes; on note_off a shaped FALL
            // (deceleration over stroke_fall_t). Short storm notes get the note_off
            // during/just after the rise -> rise+fall composes a natural per-stroke ARCH.
            if !self.releasing {
                if self.note_time < self.stroke_rise_t {
                    let x = (self.note_time / self.stroke_rise_t).min(1.0);
                    self.stroke = 0.3 + 0.7 * (x * std::f32::consts::FRAC_PI_2).sin();
                } else {
                    let t = self.note_time - self.stroke_rise_t;
                    let breathe = self.stroke_floor
                        + (1.0 - self.stroke_floor) * (-t / self.stroke_tau).exp();
                    // long notes LIVE: slow swell (+12%) starting ~0.4 s in, alongside
                    // the vibrato fade-in (sustained notes must not hold a flat level).
                    let swell = if self.note_time > 0.4 {
                        1.0 + 0.12 * (1.0 - (-(self.note_time - 0.4) / 0.8).exp())
                    } else {
                        1.0
                    };
                    self.stroke = (breathe * swell).min(1.05);
                }
            } else {
                // shaped fall: the bow decelerates (no abrupt mute)
                self.stroke *= (-dt / (self.stroke_fall_t / 3.0)).exp();
            }

            // Bow-change FORCE DIP (Demoucron Ch.6): force drops to ~0.25 at the change
            // then snaps back over ~15 ms, so the string keeps Helmholtz across the
            // reversal instead of choking/decaying.
            self.force_dip += (1.0 - self.force_dip) * (dt / 0.015).min(1.0);
            // Force follows the gesture SUPRALINEARLY (stroke^1.3): the note is darker
            // at its edges and brightest at its peak -- loudness/brightness co-variation,
            // the signature of a driven (bowed) tone vs a static reed. The target tracks
            // in ~1 string period so the Helmholtz corner stays formed (no slow sweep:
            // the gesture is the *musical* shape, the corner itself establishes fast).
            let target = if self.releasing {
                self.bow_force_target * self.stroke.powf(1.5)
            } else {
                self.bow_force_target * bow_flux * self.force_dip * self.stroke.powf(1.3)
            };
            let coeff = (dt / (1.0 / self.freq_hz).clamp(0.004, 0.020)).min(1.0);
            self.bow_force += (target - self.bow_force) * coeff;

            // Bow VELOCITY follows the gesture (loudness ~ |v_bow|). On a détaché
            // reversal it decelerates THROUGH zero over ~18 ms (the bow-change stop).
            let vel_target = self.bow_vel_target * self.stroke;
            let vel_ramp_t = if self.bow_vel * self.bow_vel_target < 0.0 { 0.018 }
                else { (1.0 / self.freq_hz).clamp(0.004, 0.020) };
            let vcoeff = (dt / vel_ramp_t).min(1.0);
            self.bow_vel += (vel_target - self.bow_vel) * vcoeff;

            // amp_env is now ONLY a ~5 ms anti-click ramp; the musical loudness shape
            // is the gesture (ONE envelope, through the physics).
            if !self.releasing {
                self.amp_env = (self.amp_env + dt / 0.005).min(1.0);
            }

            // Living vibrato: delayed fade-in, ~6 Hz FM, but humanized so it never
            // sits perfectly still -- slow wander of rate/depth, a fast flutter, and
            // a non-sinusoidal shape (real violin vibrato is asymmetric).
            self.wander += (self.hum.next() * 0.5 - self.wander) * 0.03; // slow drift
            self.flutter += (self.hum.next() - self.flutter) * 0.30; // fast micro-jitter
            let rate = patch.vib_rate * self.vib_rate_jit * (1.0 + self.wander * 0.07);
            self.vib_phase += rate * dt;
            if self.vib_phase >= 1.0 {
                self.vib_phase -= 1.0;
            }
            let vib_env = ((self.note_time - patch.vib_delay) / 0.4).clamp(0.0, 1.0);
            let ph = self.vib_phase * std::f32::consts::TAU;
            // sine + a touch of 2nd harmonic -> the asymmetric violinist vibrato shape
            let lfo = ph.sin() + 0.13 * (ph * 2.0).sin();
            let depth = patch.vib_depth * self.vib_depth_jit * self.vib_amt * (1.0 + self.wander * 0.12);
            // 1/f pitch jitter (natural micro-detuning) on top of the vibrato.
            let jitter = self.pink_pitch.next() * 3.5 * alive; // +/-~3.5 cents, pink (register-scaled)
            // SLOW INTONATION DRIFT (ensemble only, research param): each
            // player wanders independently in/out of tune over seconds (~0.4
            // Hz one-pole on white -> ~6 cents RMS, peaks ~±18c). This is the
            // measured 20-30c inter-player F0 dispersion's TIME-VARYING part,
            // the cue that makes partials continuously cross (a real section)
            // rather than sit in a static detuned chord (a fat unison).
            let drift_cents = if patch.ensemble >= 1.5 {
                self.drift += (self.hum.next() - self.drift) * 5.24e-5; // ~0.4 Hz
                self.drift * 2000.0
            } else {
                0.0 // solo: no drift, and don't perturb the hum stream
            };
            let cents = lfo * depth * vib_env + self.flutter * 0.8 * alive + jitter + drift_cents;
            self.bend = 2f32.powf(cents / 1200.0);
            // LEGATO GLIDE: ramp freq_hz toward freq_target over ~20 ms (the smooth-slur
            // crossfade window, Jaffe & Smith) instead of jumping (= spurious pluck).
            // FAST glide (~6 ms): a clean finger change, not an audible portamento. A
            // 20 ms glide over a run of slurs read as a THEREMIN slide.
            let gliding = (self.freq_hz - self.freq_target).abs() > 0.05;
            if gliding {
                self.freq_hz += (self.freq_target - self.freq_hz) * (dt / 0.006).min(1.0);
            }
            // Modal mode frequencies follow freq_hz*bend (vibrato + glide). Recompute every
            // control tick WHILE gliding (smooth slur); otherwise THROTTLE to every 8th tick
            // (recomputing ~90 modes of sin/cos/exp is the dominant CPU cost; ~187 Hz is
            // ample for a ~6 Hz vibrato).
            self.modal_recalc = self.modal_recalc.wrapping_add(1);
            if !self.releasing && (gliding || self.modal_recalc % 8 == 0) {
                let (b1, b2, stiff, beta) = Self::modal_params(self.inst_idx, self.freq_hz);
                self.modal.set_voice(self.freq_hz * self.bend, b1, b2, stiff, beta);
            }
            // 1/f amplitude shimmer (register-scaled: steady in the bass).
            self.shimmer = 1.0 + self.pink_amp.next() * 0.09 * alive;

            // Rosin scratch tracks bow speed/force, and BURSTS at the onset (the
            // bow catching the string before Helmholtz settles).
            let attack_burst = 1.0 + 3.5 * (-(self.note_time / 0.03)).exp();
            // Damp the sustained rosin hiss in the low register (real bass is ~16 dB
            // cleaner between harmonics); keep the onset burst for the bow "catch".
            self.noise_amt = patch.bow_noise * self.bow_force * (self.bow_vel.abs() + 0.04)
                * attack_burst * (0.3 + 0.7 * alive);
            // Onset CATCH transient, INDEPENDENT of the sustain noise level (which is
            // near zero on the low strings): a short (<=20 ms) burst marking the note
            // START -- the key psychoacoustic timing cue of a bow stroke (Serafin).
            // Faster rises (accents) catch harder.
            // ...a MARKER, not the loudest instant of the note (a too-strong burst put
            // the energy peak at the very onset = percussive, killing the arch shape).
            let catch = (0.02 + 0.10 * self.vel) * (0.02 / self.stroke_rise_t.max(0.008)).min(1.2);
            self.noise_amt += catch * (-(self.note_time / 0.012)).exp() * if self.releasing { 0.0 } else { 1.0 };
        }
        self.ctrl_counter = (self.ctrl_counter + 1) % CTRL_INTERVAL;

        // --- audio rate ---
        // No force floor: on note-off the bow force ramps to 0 so the bow truly
        // LIFTS and the string decays through its own losses to silence. A floor
        // here kept exciting the string forever -> a continuous drone after the
        // note stopped.
        let bridge = {
            // Demoucron modal string. CRITICAL: the bow force must be scaled to the
            // string's numerical impedance (C01 ~ 630-940) for the string to STICK and
            // form clean Helmholtz motion -- a fixed small force just slips chaotically
            // (= white noise). fb = press * C01 * |v_bow|, press in the Schelleng window.
            let press = (self.bow_force * self.modal_fscale).clamp(0.5, 2.5);
            let fb = press * self.modal.impedance() * self.bow_vel.abs();
            // bow-off: add extra modal decay so détaché notes settle (~150 ms) instead
            // of ringing like a pluck. While bowing, natural ring (1.0).
            // Ring-out damping only AFTER the shaped fall has played out (stroke low):
            // during the fall the bow is still on the string, decelerating -- damping the
            // modes then would make the end abrupt again.
            self.modal.release_damp =
                if self.releasing && self.stroke < 0.12 { self.modal_release_factor } else { 1.0 };
            self.modal.process(self.bow_vel, fb) * self.modal_gain * self.modal_pitch_gain
        };

        // Bow scratch noise, bandpassed (one-pole HP via diff of LP), injected
        // into the bridge force before the body radiates it.
        let n = self.noise.next();
        let n2 = self.noise.next();
        // GRAIN: the bow texture isn't a smooth hiss. A low "grit" band (~520 Hz =
        // the bow grabbing the string) + the ~2.8 kHz rosin scratch + a little air,
        // amplitude-GRAINED by a fast random envelope so it crackles/breathes like
        // real rosin rather than sounding like added white noise.
        self.noise_lp += (n - self.noise_lp) * 0.5;
        let grit = self.grit_bp.process(n2);
        let grain_am = 0.5 + 0.5 * (n2 * n).abs();
        let scratch = ((self.noise_bp.process(n) * 0.6 + grit * 0.5 + (n - self.noise_lp) * 0.12)
            * grain_am) * self.noise_amt * 1.4;

        // BOW IDENTITY: the rosin scratch + bow-grip attack burst is a big part of what
        // says "bowed string" rather than a free reed / accordion. The bridge is
        // gain-calibrated (~0.3), so the scratch is mixed in at the calibrated 0.4
        // (2.2 was a +10 dB 2-8 kHz harmonica buzz that spiked the onset catch).
        let excited = bridge + scratch * self.modal_scratch;
        let radiated = self.body.process(excited);

        // Track energy for activity gating.
        self.energy += (radiated.abs() - self.energy) * 0.001;
        if self.note.is_none() && self.bow_force < 1e-4 && self.energy < 1e-4 {
            self.string.reset();
            self.body.reset();
            self.energy = 0.0;
        }

        // Quadratic soft-start on the attack ramp (gentle bow catch, then builds),
        // plus the 1/f amplitude shimmer (psychoacoustic aliveness).
        // output_level is applied ONCE, by the engine's voice-sum norm
        // (engine.rs `norm`); applying it here too squared the OUT knob.
        radiated * (self.amp_env * self.amp_env) * self.shimmer
    }
}
