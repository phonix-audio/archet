//! One voice: a modal string under a bow or a finger, through a body.
//!
//! NoteOn engages the bow, whose force and speed follow a stroke gesture;
//! NoteOff lifts it and the string decays through its own losses. A
//! plucked note releases the string from a displaced shape and lets it
//! ring until a finger or the hand stops it. There is no amplitude
//! envelope on the tone: the loudness is the bow's.

use super::body::{Biquad, ModalBody};
use super::patch::ArchetPatch;
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

/// The bow force in Schelleng's terms (force over the string's impedance
/// times the bow speed) at the softest and the loudest dynamic. Measured on
/// the held-note diagnostic: Helmholtz motion settles within half a second
/// from about two and stays clean past seven.
const PRESS_SOFT: f32 = 2.0;
const PRESS_LOUD: f32 = 4.0;

/// The bowed bridge force's share of the working level: with the force in
/// its window a voice carries a full Helmholtz amplitude, and one voice at
/// full velocity must peak where the engine's voice-sum norm leaves an
/// eight-voice chord under full scale.
const BOW_LEVEL: f32 = 0.5;

/// The bow speed range over the whole velocity range, in dB: a violin's
/// dynamic range from its softest to its loudest (Meyer, Acoustics and the
/// Performance of Music, about 25 dB; the anechoic recordings put 16 dB
/// between pp and ff).
const SPEED_RANGE_DB: f32 = 26.0;

/// The cycle-to-cycle spread of a violin vibrato's period and extent, as
/// fractions, measured on a recorded held note with a normal vibrato
/// (Philharmonia Orchestra sound samples, violin B flat 4, long, arco).
const VIB_PERIOD_SPREAD: f32 = 0.033;
const VIB_EXTENT_SPREAD: f32 = 0.10;

/// The share of the steady extent a vibrato has on its first cycle, the
/// rest arriving over the next cycles; measured on four recorded held
/// notes at stopped pitches (Philharmonia Orchestra sound samples, violin,
/// long, arco with normal vibrato), whose first cycle spans 58, 69 and 137
/// percent of the steady extent where it can be read, with no delay
/// before it on any of them.
const VIB_FIRST_CYCLE: f32 = 0.65;

/// The damping setting at which the string's loss law is as calibrated.
const LOSS_NEUTRAL: f32 = 0.30;

/// Where a finger plucks, as a fraction of the open string's length: the
/// median the recorded open strings imply from their fifth partial.
pub const PLUCK_SPOT: f32 = 0.185;

/// Slow intonation drift: the corner of a one-pole walk and its rms in
/// cents, the same for a soloist and for each player of a section
/// (recorded held notes move by one to three cents rms below 1.5 Hz).
const DRIFT_HZ: f32 = 0.4;
const DRIFT_CENTS: f32 = 2.0;

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
        for (i, (s, a)) in self.s.iter_mut().zip(A.iter()).enumerate() {
            let w = self.n.next();
            *s += (w - *s) * a;
            sum += *s * (1.0 - i as f32 * 0.12);
        }
        self.out = (sum * 0.55).clamp(-1.0, 1.0);
        self.out
    }
}

#[derive(Debug, Clone)]
pub struct ArchetVoice {
    modal: ModalString,   // Demoucron modal string (the one production model)
    // The second transverse plane of a plucked string, a fraction sharp of
    // the first: every partial of a recorded pluck is a doublet that beats.
    modal_b: ModalString,
    // A shaped excitation buffer fed into the string sample by sample
    // (Vaelimaeki 2004).
    modal_gain: f32,      // output calibration gain for the modal bridge force
    modal_scratch: f32,   // rosin-scratch mix into the modal bridge (calibrated 0.4)
    modal_recalc: u32,    // throttles the vibrato coefficient recompute (CPU)
    modal_release_factor: f32, // per-sample modal decay on bow-off (~150 ms detache)
    modal_pitch_gain: f32, // pitch-compensating output gain (low notes are ~10 dB too loud)
    body: ModalBody,
    // The (inst, detune, bridge) the current `body` was laid out with. The
    // body is deterministic in these, so it is laid out again only when
    // they change and otherwise has its filter state reset in place.
    body_inst: usize,
    body_detune: f32,
    body_bridge: f32,
    noise: Noise,
    noise_lp: f32,
    noise_bp: Biquad, // bandpass shaping the rosin scratch (~2.8 kHz)
    grit_bp: Biquad,  // low "grit" band (~520 Hz) -- the bow grabbing the string
    // humanization (separate noise stream so it doesn't colour the bow scratch)
    hum: Noise,
    bow_beta: f32,        // the bow's point on the string, a fraction of its length
    string_damping: f32,  // scale on the string's loss law
    vib_cycle_rate: f32,  // this vibrato cycle's rate, around the note's
    vib_cycle_depth: f32, // this vibrato cycle's extent, around the note's
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
    hand_damp: f32, // per-sample modal decay once the key is up and the hand mutes

    pub note: Option<u8>,
    /// Monotonic note-on sequence number, set by the engine: a note-off
    /// releases only the oldest voice of a pitch, so on overlapping repeated
    /// notes the previous note's off does not end the fresh one.
    pub on_seq: u64,
    inst_idx: usize, // per-note instrument body (0 violin..3 bass), from pitch if auto_range
    vel: f32,
    sr: f32,
    voice_idx: usize,

    // bow state
    bow_vel_target: f32,
    bow_force_target: f32,
    bow_force: f32, // smoothed (the attack/release ramp)
    bow_dir: f32,   // bow direction +1/-1; flips at a detache bow change
    force_dip: f32, // transient bow-force reduction at a bow change (ramps back to 1)
    // The bow-stroke gesture: the within-note shape of bow speed, a rise,
    // a held sustain and a shaped fall (Demoucron, chapters 5-6). The bow
    // force follows it, so loudness and brightness co-vary through the note.
    stroke: f32,        // current gesture value (multiplies bow_vel/force targets)
    stroke_rise_t: f32, // per-note rise time (8-60 ms; accents fast, soft notes slow)
    stroke_fall_t: f32, // per-note fall time on note_off (50-90 ms; longer when low)
    stroke_floor: f32,  // sustain decay floor (low strings breathe more)
    stroke_tau: f32,    // sustain decay time constant (s)
    releasing: bool,
    amp_env: f32,   // amplitude attack envelope (soft bow onset, 0->1)
    freq_hz: f32,     // current sounding frequency (sizes the bow-force establishment time)
    freq_target: f32, // legato glide target (freq_hz ramps to it over ~20 ms = slur crossfade)
    vib_amt: f32,     // per-note vibrato scale (soft/short notes vibrate less)

    // vibrato
    vib_phase: f32,
    note_time: f32,

    // String section: this player's static F0 offset (cents), stage azimuth,
    // onset asynchrony, and a slow independent intonation drift.
    unison_det: f32,
    unison_pan: f32,
    onset_delay: usize, // samples to wait before this player's bow engages
    release_delay: i32, // samples until this player lifts the bow (-1 = none);
                        // staggers a section's note-offs (bows lift apart)
    drift: f32,         // slewed slow random-walk pitch drift (raw, ~+/-0.003)

    // activity tracking
    energy: f32,

    // Steal declick: when a sounding voice is retriggered (a voice steal or
    // a same-note reuse), the old sound fades over about 3 ms before the
    // new note resets the modal state.
    stealing: bool,
    steal_fade: f32,
    steal_pending: Option<(u8, u8)>, // (note, vel) queued behind the fade
    /// The queued note's ensemble onset delay (set_unison runs before
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
            modal: ModalString::new(sr),
            modal_b: ModalString::new(sr),
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
            body: ModalBody::new(sr, 0, 1.0, 9.0),
            body_inst: 0,
            body_detune: 1.0,
            body_bridge: 9.0,
            noise: Noise::new(0x9E37_79B9 ^ ((voice_idx as u32).wrapping_mul(2654435761))),
            noise_lp: 0.0,
            noise_bp: Biquad::bandpass(sr, 2800.0, 1.1),
            grit_bp: Biquad::bandpass(sr, 520.0, 0.9),
            hum: Noise::new(0x1234_5678 ^ ((voice_idx as u32).wrapping_mul(40503))),
            bow_beta: 0.075,
            string_damping: 1.0,
            vib_cycle_rate: 1.0,
            vib_cycle_depth: 1.0,
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
            hand_damp: 1.0,
            stroke: 0.3,
            stroke_rise_t: 0.03,
            stroke_fall_t: 0.06,
            stroke_floor: 0.8,
            stroke_tau: 1.2,
            freq_hz: 220.0,
            freq_target: 220.0,
            releasing: false,
            amp_env: 0.0,
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

    /// Set the unison player's static F0 offset (cents), stage pan and onset
    /// delay (samples) before a note-on (the section's scatter and bow
    /// asynchrony; see the engine's fire_unison). Persists across note_on;
    /// reset to (0, 0, 0) for ordinary notes.
    pub fn set_unison(&mut self, det_cents: f32, pan: f32, onset_delay: usize) {
        self.unison_det = det_cents;
        self.unison_pan = pan;
        self.onset_delay = onset_delay;
    }

    /// Schedule this player's bow lift `delay` samples from now (a section's
    /// players do not stop together). A delay of 0 releases at once.
    pub fn schedule_release(&mut self, delay: i32, patch: &ArchetPatch) {
        if delay <= 0 {
            self.note_off(patch);
        } else {
            self.release_delay = delay;
        }
    }

    /// Re-seed every stochastic stream so this voice is independent of the
    /// same-index voice in another engine instance (ArchetPatch::seed_offset).
    /// Safe to call while idle, before any NoteOn.
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
        // The sub-bass below 70 Hz is held down.
        let sub = (freq / 70.0).min(1.0).powf(0.5);
        g * sub
    }
    /// The instrument a note is played on: in `auto_range` (ensemble) mode
    /// it follows the note's pitch, so one engine renders a full-range desk;
    /// otherwise it is the patch's. Bands: G3 (55) and above violin, C3 to
    /// F#3 viola, C2 to B2 cello, below C2 bass.
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
    pub(crate) fn measured_pizz_t60(body_index: usize, string: usize) -> &'static [(f32, f32)] {
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
        tables[string.min(3)]
    }

    /// How rounded the released shape is, per string, as the corner of the
    /// one-pole weight `release` applies to the modes. Calibrated on the same
    /// anechoic recordings as the decays: the corner that gives the rendered
    /// attack, body included, the spectral centroid the recorded open string
    /// has in its first fifty milliseconds (a hundred on cello and bass).
    ///
    /// It is not a finger width. An ideal triangle released at a point puts
    /// far more energy in the upper partials than any recorded pluck, and
    /// neither a rectangular nor a soft contact of any plausible width brings
    /// it down: the widths those forms demand run to tens of centimetres. So
    /// the number is what it is, the rounding a string needs, and it is
    /// tightest on the lowest string of each instrument. Where the recording
    /// is as bright as the model, no corner applies. On the bass, three
    /// strings hold an uncalibrated corner: the fit there runs below the
    /// fundamental, where a corner only adds the release scrape and the
    /// body's knock.
    fn attack_corner(body_index: usize, string: usize) -> f32 {
        const HZ: [[f32; 4]; 4] = [
            [900.0, 1000.0, 20000.0, 20000.0], // violin G D A E
            [566.0, 1091.0, 1246.0, 1573.0],  // viola C G D A
            [463.0, 1496.0, 2086.0, 1857.0],  // cello C G D A
            [2276.0, 3039.0, 1593.0, 5414.0], // double bass E A D G, D alone calibrated
        ];
        HZ[body_index.min(3)][string.min(3)]
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
    /// The instruments' relative levels: the melody instruments forward,
    /// the bass instruments back.
    /// The four instruments at the same loudness: a violin, a viola, a
    /// cello and a double bass play at comparable sound levels (Meyer,
    /// Acoustics and the Performance of Music), and a desk's balance is
    /// the player's level control, not the instrument's. The figures undo
    /// what each measured body takes from the string's level, so a note at
    /// the same velocity reads the same on each.
    fn inst_level(idx: usize) -> f32 {
        match idx {
            0 => 1.00,
            1 => 0.97,
            2 => 1.45,
            _ => 2.85,
        }
    }
    /// The share of samples the bowed string spent slipping since the last
    /// call, with the bow's force and speed now, for diagnostics.
    #[cfg(test)]
    pub(crate) fn bow_state(&mut self) -> (f32, f32, f32) {
        let frac = self.modal.slips as f32 / self.modal.steps.max(1) as f32;
        self.modal.slips = 0;
        self.modal.steps = 0;
        (frac, self.bow_force, self.bow_vel)
    }

    /// The velocity as the patch lets it act: its distance from mezzo-forte
    /// scaled by the share of the dynamic range the patch gives it.
    fn dynamic(patch: &ArchetPatch, v: f32) -> f32 {
        0.5 + patch.vel_sens.clamp(0.0, 1.0) * (v - 0.5)
    }

    fn modal_params(body_index: usize, freq: f32) -> (f32, f32, f32, f32) {
        // Per instrument: b1 and b2, the per-partial damping law's base and
        // growth; the stiffness inharmonicity, of the order of 1e-5; and the
        // bow position beta.
        let (b1, b2, stiff, beta) = match body_index {
            0 => (1.6, 0.045, 2.0e-5, 0.075), // violin
            1 => (1.4, 0.055, 3.0e-5, 0.080), // viola
            2 => (1.2, 0.060, 4.0e-5, 0.082), // cello
            _ => (1.0, 0.075, 6.0e-5, 0.082), // double bass
        };
        // Per-partial damping grows with pitch, so high notes, which have few
        // modes, stay clean.
        let b2 = b2 * (1.0 + (freq / 750.0).powi(2));
        let b1m = 1.0f32;
        let b2m = 1.0f32;
        (b1 * b1m, b2 * b2m, stiff, beta)
    }

    fn freq_tuned(note: u8, patch: &ArchetPatch) -> f32 {
        Self::freq_of(note) * 2f32.powf(patch.tune_cents / 1200.0)
    }

    /// Per-note expression, shared by fresh bows and legato retunes: velocity
    /// and pitch set the brightness, the loudness, the attack and the
    /// vibrato amount.
    fn apply_expression(&mut self, note: u8, v: f32, patch: &ArchetPatch) {
        let v = Self::dynamic(patch, v);
        // Pitch-dependent brightness (octaves above D4=62; Schelleng) + velocity
        // brightness (pp dark/flautando, ff brilliant).
        let oct = (note as f32 - 62.0) / 12.0;
        let bright = (1.0 + oct * 0.45).clamp(0.6, 2.6);
        // The three controls the string reads: where the bow crosses it,
        // how damped it is about its calibrated law, and how much of the
        // dynamic range the velocity commands.
        self.bow_beta = patch.bow_pos.clamp(0.03, 0.2);
        self.string_damping = (patch.loss / LOSS_NEUTRAL).clamp(0.25, 2.0);

        // The bow force in Schelleng's terms, over the string's impedance
        // times the bow speed. The dynamic moves it through the lower half
        // of the window where the string settles into Helmholtz motion
        // within half a second and stays clean, louder notes pressing more.
        let press = PRESS_SOFT + (PRESS_LOUD - PRESS_SOFT) * v;
        let r1 = self.hum.next();
        let r2 = self.hum.next();
        let r3 = self.hum.next();
        let r4 = self.hum.next();
        let r5 = self.hum.next();
        let r6 = self.hum.next();
        // In a section every player differs in attack speed, release speed,
        // bow force and vibrato as well as pitch (Meyer: a section's traits
        // are desynchronized vibrato, spread attacks and releases, and
        // per-player dynamics); `js` widens the per-voice spread there. A
        // solo keeps the tight spread.
        let ens = patch.ensemble >= 1.5;
        let js = if ens { 2.4 } else { 1.0 }; // per-voice variation scale
        self.bow_force_target = patch.bow_force * press * (1.0 + r3 * 0.07 * js);
        // Loudness follows the bow speed (Helmholtz amplitude is the speed
        // over the bow position), so the dynamic is a speed, logarithmic
        // in the velocity over the instrument's whole range.
        let speed = 10f32.powf((v - 1.0) * SPEED_RANGE_DB / 20.0);
        self.bow_vel_target = patch.bow_vel * speed * bright.sqrt() * (1.0 + r4 * 0.08 * js);
        // desynchronized, continuous, deeper vibrato for the section
        if ens {
            self.vib_amt = (0.70 + 0.30 * v) * (1.0 + r2 * 0.15);
            self.vib_rate_jit = 1.0 + r1 * 0.22;          // +/-22% rate (~4.3-6.7 Hz)
            self.vib_depth_jit = (1.0 + r2 * 0.40) * 1.4; // wider + deeper extent
        } else {
            self.vib_amt = (0.35 + 0.8 * v) * (1.0 + r2 * 0.15);
            self.vib_rate_jit = 1.0 + r1 * 0.10;
            self.vib_depth_jit = 1.0 + r2 * 0.20;
        }
        // The per-note gesture: rise and fall times jittered per voice,
        // widened for a section so no two strokes share a shape.
        let lowi = self.inst_idx >= 2;
        // The bow's acceleration: from rest to its speed over the attack
        // setting, quicker on an accent (Guettler, On the creation of the
        // Helmholtz motion in bowed strings, Acustica 2002: the attack is a
        // ramp of speed under a force that is there first).
        self.stroke_rise_t = (patch.attack * (1.6 - 0.8 * v) * (1.0 + r5 * 0.25 * js)).clamp(0.005, 0.3);
        // The release control sets the gesture's fall as well: what a
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
        // Steal declick: a fresh attack resets the modal string and the
        // amplitude, an instant step from whatever this voice radiates. If
        // the voice still sounds, the note is queued behind a fade of about
        // 3 ms instead (see process()).
        if self.is_active() && (self.energy > 1e-3 || self.stealing) {
            if !self.stealing {
                self.stealing = true;
                self.steal_fade = 1.0;
            }
            self.steal_pending = Some((note, vel));
            // set_unison ran before this note_on: park the new note's onset
            // delay so render() keeps playing the old sound during the fade.
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
        let v = Self::dynamic(patch, (vel as f32 / 127.0).clamp(0.0, 1.0));
        self.vel = v;
        self.note = Some(note);
        self.release_delay = -1; // fresh/retriggered note: cancel any pending lift
        self.inst_idx = Self::inst_for(note, patch);
        self.releasing = false;
        self.amp_env = 0.0; // start silent -> the bow attack ramps the loudness in
        self.note_time = 0.0;
        // A random vibrato phase per note, so a section's players vibrate
        // independently.
        self.vib_phase = self.hum.next() * 0.5 + 0.5;
        self.energy = 0.0;

        // A per-voice and per-desk body detune, so a section is many distinct
        // instruments. tune_cents differs per desk.
        let detune = 1.0 + ((self.voice_idx as f32 * 0.013).sin()) * 0.012 + patch.tune_cents * 0.0007;
        // Lay the body out again only when its inputs change; otherwise reset
        // its state in place, which gives the same coefficients.
        if self.inst_idx != self.body_inst
            || (detune - self.body_detune).abs() > 1e-9
            || (patch.bridge_hill_db - self.body_bridge).abs() > 1e-9
        {
            self.body.rebuild(self.sr, self.inst_idx, detune, patch.bridge_hill_db);
            self.body_inst = self.inst_idx;
            self.body_detune = detune;
            self.body_bridge = patch.bridge_hill_db;
        } else {
            self.body.reset();
        }

        self.apply_expression(note, v, patch);
        self.bow_dir = 1.0;       // fresh stroke: bow drawn in the reference direction
        self.force_dip = 1.0;
        // unison_det: this player's static section detune (cents) on top of
        // the patch and desk tuning, the frequency scatter Ternstroem measures.
        self.freq_hz = Self::freq_tuned(note, patch) * 2f32.powf(self.unison_det / 1200.0);
        self.freq_target = self.freq_hz; // no glide on a fresh attack
        self.modal_pitch_gain = Self::modal_pgain(self.freq_hz) * Self::inst_level(self.inst_idx);

        // The plucked articulation: a release of the string, then a free
        // modal ring-down; the bow is bypassed (see process()).
        if patch.pluck {
            // The pluck (Vaelimaeki and Penttinen, EURASIP 2004, for the
            // architecture):
            // (a) pluck point: the hand sits at the end of the fingerboard
            // whatever the note, about a fifth of the open length from the
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
            // What is measured is that the position moves: between the fingers
            // of one player it spans up to one percent of the string length, and
            // its variance grows with register (Chadefaux, Le Carrou and Fabre,
            // Experimentally-based description of harp plucking, JASA 2012). So
            // draw it per note over that span, which leaves the comb in place
            // for each note and moves it from note to note, as a hand does. No
            // published measurement pins the spot for a violin; the depth of
            // the fifth partial on the four recorded open strings, never
            // absent, puts it between a sixth and a fifth of the length, and
            // the median of the four, PLUCK_SPOT, is what the plucked presets
            // carry as their position; the patch's position is the finger's.
            // The spread is half a percent each way, and the fraction folds
            // about the middle, where the comb is symmetric.
            let (string, f_open, _) = Self::string_for(self.inst_idx, note);
            self.on_string = Some((self.inst_idx, string));
            self.stopped = false;
            let spot = patch.bow_pos.clamp(0.03, 0.5) * Self::freq_of(note) / f_open;
            let spot = if spot > 0.5 { 1.0 - spot } else { spot };
            let p = (spot + self.hum.next() * 0.005).clamp(0.02, 0.5);
            // (b) the same string the bow uses, with the same stiffness law.
            let (_, _, stiff, _) = Self::modal_params(self.inst_idx, self.freq_hz);
            // (c) per-partial decay: the measured curve of the string this
            // note is on, read at each partial's own frequency, stiffness
            // included (see `measured_pizz_t60`).
            let f0 = self.freq_hz;
            let measured = Self::measured_pizz_t60(self.inst_idx, string);
            // The damping setting scales the measured decays, as it scales
            // the bowed string's losses.
            let damping = (patch.loss / LOSS_NEUTRAL).clamp(0.25, 2.0);
            let t60law = move |k: usize| -> f32 {
                let kf = k as f32;
                let sk = stiff * kf * kf;
                Self::t60_from_table(measured, f0 * kf * (1.0 + sk).sqrt()) / damping
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
            // (d) the pluck is a release. The finger pulls the string aside
            // and lets go, so the modes start at a displacement that falls as
            // 1/n^2 and at zero velocity. Harder plucking pulls the string
            // further, so the velocity sets the displacement rather than a
            // force. The lowpass is the fingertip's own compliance, rounding
            // the corner of the triangle.
            let h = 5200.0f32 * (0.35 + 0.65 * self.vel);
            // The rounding of the released shape, per string, calibrated on
            // the recordings (see `attack_corner`): fixed in hertz on each
            // string, so a note stopped higher on it is rounder still.
            let plp = Self::attack_corner(self.inst_idx, string);
            // The release's gain is a voicing constant, measured rather than
            // derived: it puts the note at the peak the bank was voiced
            // against (0.0565 on the repeat profile at D4, velocity 100).
            let amp = h * 116.0 * (1.0 + self.hum.next() * 0.05);
            self.modal.release(amp, plp, self.freq_hz);
            self.modal_b.release(amp * PLANE_LEVEL, plp, self.freq_hz * (1.0 + PLANE_DETUNE));
            self.bow_force = 0.0;
            self.bow_vel = 0.0;
            self.amp_env = 1.0;
            return;
        }

        // The start (Guettler 2002; Woodhouse, Euphonics 9.5): the stroke
        // gesture starts at 0.3, the bow never catching at full speed, and
        // rises over stroke_rise_t; the modal force follows the bow speed, so
        // the Schelleng ratio stays playable through the rise.
        self.stroke = 0.0;
        // The force is consistent with the gesture (stroke^1.3) from the
        // first sample; the corner forms within about a period.
        // Force first, the bow at rest on the string; the speed follows.
        self.bow_force = self.bow_force_target;
        self.bow_vel = 0.0;

        {
            let (b1, b2, stiff, _) = Self::modal_params(self.inst_idx, self.freq_hz);
            let (b1, b2, beta) = (b1 * self.string_damping, b2 * self.string_damping, self.bow_beta);
            // Slip noise: a touch on the violin, near none on the low strings.
            self.modal.noise_amt = match self.inst_idx { 0 => 0.12, 1 => 0.05, _ => 0.02 };
            self.modal.set_voice(self.freq_hz, b1, b2, stiff, beta);
            self.modal.reset(); // fresh bow stroke: silent string, perfect-start bow catches it
        }

        self.vib_cycle_rate = 1.0;
        self.vib_cycle_depth = 1.0;
        self.flutter = 0.0;
    }

    /// A slur on the same string: the bow keeps going in the same direction
    /// and only the finger changes pitch. freq_hz glides to the new pitch
    /// rather than jumping (an abrupt retune is Jaffe and Smith's spurious
    /// pluck); the amplitude and the bow direction continue.
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
    }

    /// A detache bow change: the bow stays in contact and reverses; the
    /// force dips briefly at the velocity zero-crossing and comes back, so
    /// Helmholtz motion transfers reversed instead of decaying (Demoucron,
    /// chapter 6). The amplitude continues; there is no re-attack.
    pub fn bow_change(&mut self, note: u8, vel: u8, patch: &ArchetPatch) {
        let v = (vel as f32 / 127.0).clamp(0.0, 1.0);
        self.vel = v;
        self.note = Some(note);
        self.release_delay = -1; // fresh/retriggered note: cancel any pending lift
        self.inst_idx = Self::inst_for(note, patch);
        self.releasing = false;
        self.apply_expression(note, v, patch);
        self.bow_dir = -self.bow_dir;            // reverse the bow
        self.bow_vel_target *= self.bow_dir;     // bow_vel ramps through zero
        self.force_dip = 0.18;                   // force drops at the change, then ramps back to 1
        self.note_time = 0.0;                    // re-fire the bow-grip noise burst (detache attack)
        self.freq_hz = Self::freq_tuned(note, patch); // detache: clean re-pitch (separate note)
        self.freq_target = self.freq_hz;
        self.modal_pitch_gain = Self::modal_pgain(self.freq_hz) * Self::inst_level(self.inst_idx);
        {
            let (b1, b2, stiff, _) = Self::modal_params(self.inst_idx, self.freq_hz);
            let (b1, b2, beta) = (b1 * self.string_damping, b2 * self.string_damping, self.bow_beta);
            self.modal.set_voice(self.freq_hz, b1, b2, stiff, beta);
            self.modal.soften(0.5); // the reversal re-forms the corner; shed stale energy
        }
    }

    pub fn note_off(&mut self, patch: &ArchetPatch) {
        // A note released while its steal-fade is still running never
        // started sounding - cancel the pending attack (the fade keeps
        // running to zero, then the voice silences; see process()).
        self.steal_pending = None;
        // The release control is in seconds and a bow's release is a
        // per-sample modal decay, so one is derived from the other at every
        // note-off. A plucked string is untouched: `process` returns on that
        // path before this factor applies, and a finger leaving a string
        // damps nothing.
        let secs = patch.release.clamp(0.01, 1.0);
        self.modal_release_factor = (0.1f32).powf(1.0 / (secs * self.sr));
        // A plucked string rings by its own losses only while the key is
        // held: the player who lets a note ring holds it. Once the key is up
        // the hand mutes the string, and how fast is the same control, in
        // seconds. Notes released together, a double stop, go together.
        self.hand_damp = (0.1f32).powf(1.0 / (secs * self.sr));
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
    /// queued note fires (fresh attack from silence - the modal reset is
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

        // Onset asynchrony in a section: this player's bow lands a few ms late.
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

        // The plucked path: the string rings down and the body the bow
        // drives radiates it. On note-off the hand mutes it at the pace of
        // the release control; a finger landing on the same string stops it.
        if patch.pluck {
            // While the key is held the string rings by its measured losses.
            // A finger landing on the same string ends it at once; the hand
            // muting after the key is up ends it at the release control's
            // pace.
            let damp = if self.stopped {
                self.stop_damp
            } else if self.releasing {
                self.hand_damp
            } else {
                1.0
            };
            self.modal.release_damp = damp;
            self.modal_b.release_damp = damp;
            let bridge = (self.modal.process(0.0, 0.0) + self.modal_b.process(0.0, 0.0))
                * self.modal_gain
                * self.modal_pitch_gain;
            let out = self.body.process(bridge);
            self.energy += (out.abs() - self.energy) * 0.001;
            // output_level is applied ONCE, by the engine's voice-sum norm
            // (engine.rs `norm`); applying it here too squared the OUT knob.
            return out * self.amp_env;
        }

        // The 1/f micro-modulation and the rosin noise scale with the
        // register: a bowed bass or cello sustains far steadier than a
        // violin (a recorded contrabass note has about half the amplitude
        // wobble, a quarter of the spectral flux and 16 dB less
        // inter-harmonic noise).
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

            // 1/f modulation of the bow pressure: a slowly evolving brightness.
            let pbow = self.pink_bow.next();
            let bow_flux = 1.0 + pbow * 0.14 * alive; // +/-14% slow bow-pressure drift (register-scaled)

            // The bow's force and speed are established within about a string
            // period of the first slip, so the Helmholtz corner is formed from
            // the first cycle (Woodhouse, Euphonics 9.2 and 9.5; Guettler
            // 2002); the loudness rises separately.
            // The stroke gesture: a rise from 0.3 to 1.0 over stroke_rise_t
            // (a quarter sine), a held sustain, and on note_off a shaped fall
            // over stroke_fall_t. A short note whose note_off lands during
            // the rise composes an arch of the rise and the fall.
            if !self.releasing {
                if self.note_time < self.stroke_rise_t {
                    let x = (self.note_time / self.stroke_rise_t).min(1.0);
                    self.stroke = (x * std::f32::consts::FRAC_PI_2).sin();
                } else {
                    // A held note is held: the bow keeps its speed and force
                    // for as long as the key is down.
                    self.stroke = 1.0;
                }
            } else {
                // shaped fall: the bow decelerates (no abrupt mute)
                self.stroke *= (-dt / (self.stroke_fall_t / 3.0)).exp();
            }

            // The force dip at a bow change (Demoucron, chapter 6): the force
            // drops to about a quarter at the change and comes back over
            // about 15 ms, so the string keeps Helmholtz motion across the
            // reversal.
            self.force_dip += (1.0 - self.force_dip) * (dt / 0.015).min(1.0);
            // Force follows the gesture supralinearly (stroke^1.3): the note is
            // darker at its edges and brightest at its peak. The target tracks
            // in about a string period, so the Helmholtz corner stays formed.
            let target = if self.releasing {
                self.bow_force_target * self.stroke.powf(1.5)
            } else {
                self.bow_force_target * bow_flux * self.force_dip
            };
            let coeff = (dt / (1.0 / self.freq_hz).clamp(0.004, 0.020)).min(1.0);
            self.bow_force += (target - self.bow_force) * coeff;

            // Bow speed follows the gesture (loudness follows |v_bow|). On a
            // detache reversal it decelerates through zero over about 18 ms.
            let vel_target = self.bow_vel_target * self.stroke;
            let vel_ramp_t = if self.bow_vel * self.bow_vel_target < 0.0 { 0.018 }
                else { (1.0 / self.freq_hz).clamp(0.004, 0.020) };
            let vcoeff = (dt / vel_ramp_t).min(1.0);
            self.bow_vel += (vel_target - self.bow_vel) * vcoeff;

            // amp_env is a 5 ms anti-click ramp; the loudness shape is the
            // gesture, through the physics.
            if !self.releasing {
                self.amp_env = (self.amp_env + dt / 0.005).min(1.0);
            }

            // Vibrato: delayed fade-in; a rate and an extent drawn afresh at
            // each cycle around the note's own, by the cycle-to-cycle spread
            // of a recorded vibrato; a fast flutter; and a non-sinusoidal
            // shape (a violinist's vibrato is asymmetric).
            self.flutter += (self.hum.next() - self.flutter) * 0.30; // fast micro-jitter
            let rate = patch.vib_rate * self.vib_rate_jit * self.vib_cycle_rate;
            self.vib_phase += rate * dt;
            if self.vib_phase >= 1.0 {
                self.vib_phase -= 1.0;
                // A uniform draw in [-1, 1] has a third of unit variance.
                let unit = 3f32.sqrt();
                self.vib_cycle_rate = 1.0 + self.hum.next() * unit * VIB_PERIOD_SPREAD;
                self.vib_cycle_depth = 1.0 + self.hum.next() * unit * VIB_EXTENT_SPREAD;
            }
            // The extent from the note's start: recorded held notes carry
            // their first vibrato cycle within the first period at about
            // two thirds of the steady extent and reach it within two
            // cycles. A delay before it is the player's, and none by default.
            let since = self.note_time - patch.vib_delay;
            let vib_env = if since < 0.0 {
                0.0
            } else {
                1.0 - (1.0 - VIB_FIRST_CYCLE) * (-(since * rate)).exp()
            };
            let ph = self.vib_phase * std::f32::consts::TAU;
            // sine + a touch of 2nd harmonic -> the asymmetric violinist vibrato shape
            let lfo = ph.sin() + 0.13 * (ph * 2.0).sin();
            let depth = patch.vib_depth * self.vib_depth_jit * self.vib_amt * self.vib_cycle_depth;
            // 1/f pitch jitter (natural micro-detuning) on top of the vibrato.
            let jitter = self.pink_pitch.next() * 3.5 * alive; // +/-~3.5 cents, pink (register-scaled)
            // Slow intonation drift, a one-pole walk over seconds: a soloist
            // moves by a couple of cents rms, the players of a section, each
            // on their own, by more, so their partials keep crossing.
            let pole = std::f32::consts::TAU * DRIFT_HZ * dt;
            self.drift += (self.hum.next() - self.drift) * pole;
            let unit_rms = (pole / (2.0 - pole)).sqrt() / 3f32.sqrt();
            let drift_cents = self.drift / unit_rms
                * DRIFT_CENTS;
            let cents = lfo * depth * vib_env + self.flutter * 0.8 * alive + jitter + drift_cents;
            self.bend = 2f32.powf(cents / 1200.0);
            // The legato glide: freq_hz ramps toward freq_target over about
            // 6 ms, a finger change rather than a jump (Jaffe and Smith's
            // spurious pluck) and short of an audible portamento.
            let gliding = (self.freq_hz - self.freq_target).abs() > 0.05;
            if gliding {
                self.freq_hz += (self.freq_target - self.freq_hz) * (dt / 0.006).min(1.0);
            }
            // The mode frequencies follow freq_hz*bend (vibrato and glide):
            // recomputed every control tick while gliding, otherwise every
            // eighth tick (recomputing 90 modes is the dominant cost, and
            // about 187 Hz is ample for a 6 Hz vibrato).
            self.modal_recalc = self.modal_recalc.wrapping_add(1);
            if !self.releasing && (gliding || self.modal_recalc.is_multiple_of(8)) {
                let (b1, b2, stiff, _) = Self::modal_params(self.inst_idx, self.freq_hz);
            let (b1, b2, beta) = (b1 * self.string_damping, b2 * self.string_damping, self.bow_beta);
                self.modal.set_voice(self.freq_hz * self.bend, b1, b2, stiff, beta);
            }
            // 1/f amplitude shimmer (register-scaled: steady in the bass).
            self.shimmer = 1.0 + self.pink_amp.next() * 0.09 * alive;

            // The rosin scratch tracks bow speed and force, and bursts at the
            // onset, the bow catching the string before Helmholtz motion
            // settles.
            let attack_burst = 1.0 + 3.5 * (-(self.note_time / 0.03)).exp();
            // The sustained hiss scales with the register (a recorded bass is
            // about 16 dB cleaner between harmonics); the onset burst stays.
            self.noise_amt = patch.bow_noise * (self.bow_force / PRESS_LOUD) * (self.bow_vel.abs() + 0.04)
                * attack_burst * (0.3 + 0.7 * alive);
            // The onset catch, independent of the sustain noise level: a burst
            // of at most 20 ms marking the note's start, the timing cue of a
            // bow stroke (Serafin). Faster rises catch harder. It marks the
            // start and is not the loudest instant of the note.
            let catch = (0.02 + 0.10 * self.vel) * (0.02 / self.stroke_rise_t.max(0.008)).min(1.2);
            self.noise_amt += catch * (-(self.note_time / 0.012)).exp() * if self.releasing { 0.0 } else { 1.0 };
        }
        self.ctrl_counter = (self.ctrl_counter + 1) % CTRL_INTERVAL;

        // --- audio rate ---
        // No force floor: on note-off the bow force ramps to zero, the bow
        // lifts, and the string decays through its own losses.
        let bridge = {
            // Demoucron modal string. The bow force is scaled to the string's
            // numerical impedance and the bow speed, so the same figure sits
            // at the same place in the Schelleng window on every string and
            // note: fb = press * C01 * |v_bow|.
            let press = self.bow_force.clamp(0.5, 2.0 * PRESS_LOUD);
            // The force is the player's, not the speed's: through the attack
            // it stands at its full value while the bow accelerates, and it
            // leaves with the bow when the bow lifts.
            let speed = if self.releasing { self.bow_vel.abs() } else { self.bow_vel_target.abs() };
            let fb = press * self.modal.impedance() * speed;
            // Bow off: extra modal decay so detache notes settle in about
            // 150 ms; while bowing, the natural ring. The damping applies only
            // after the shaped fall has played out, while the bow is still on
            // the string and decelerating.
            self.modal.release_damp =
                if self.releasing && self.stroke < 0.12 { self.modal_release_factor } else { 1.0 };
            self.modal.process(self.bow_vel, fb) * self.modal_gain * BOW_LEVEL * self.modal_pitch_gain
        };

        // Bow scratch noise, bandpassed (one-pole HP via diff of LP), injected
        // into the bridge force before the body radiates it.
        let n = self.noise.next();
        let n2 = self.noise.next();
        // The bow's texture: a low grit band near 520 Hz, the rosin scratch
        // near 2.8 kHz and a little air, grained by a fast random envelope.
        self.noise_lp += (n - self.noise_lp) * 0.5;
        let grit = self.grit_bp.process(n2);
        let grain_am = 0.5 + 0.5 * (n2 * n).abs();
        let scratch = ((self.noise_bp.process(n) * 0.6 + grit * 0.5 + (n - self.noise_lp) * 0.12)
            * grain_am) * self.noise_amt * 1.4;

        // The scratch is mixed into the calibrated bridge force at its
        // calibrated level.
        let excited = bridge + scratch * self.modal_scratch;
        let radiated = self.body.process(excited);

        // Track energy for activity gating.
        self.energy += (radiated.abs() - self.energy) * 0.001;
        if self.note.is_none() && self.bow_force < 1e-4 && self.energy < 1e-4 {
            self.body.reset();
            self.energy = 0.0;
        }

        // A quadratic soft start on the attack ramp, and the 1/f amplitude
        // shimmer. output_level is applied once, by the engine's voice-sum
        // norm (engine.rs `norm`).
        radiated * (self.amp_env * self.amp_env) * self.shimmer
    }
}
