//! Procedural violin BODY for Archet (commuted synthesis, source-filter form).
//!
//! The signature-mode frequencies and their identifications are the published
//! ones (Gough, Acoustics Today 2016; Woodhouse, Rep. Prog. Phys. 2014), and
//! the band gains are aimed at Duennwald's old-Italian profile (1991, via
//! Buen) -- the `dunnwald` diagnostic below prints the three band criteria,
//! though it does not yet assert them. The result is NOT a simple lowpass --
//! it is a specific formant structure:
//!
//!   A0 air  ~280 Hz  (+)        B1- corpus ~410 Hz (++ strongest)
//!   B1+     ~620 Hz  (+)        wood peak  ~1000 Hz (+)
//!   VALLEY  ~1300-1700 Hz (-)   transition ~2000-2600 Hz
//!   peak    ~3000 Hz  (+)       roll-off   above ~3300 Hz
//!   plus radiation roll-off below the A0 air resonance.
//!
//! Implemented as a cascade of peaking EQ sections (matching that magnitude on
//! the broadband string signal, so all harmonics are preserved) bracketed by the
//! sub-A0 highpass and the HF roll-off lowpass. Fitted/validated against the
//! reference by `violin_fit`. Mode identifications after Gough (Acoustics Today
//! 2016) / Woodhouse (Rep. Prog. Phys. 2014).

use std::f32::consts::PI;

/// The body's biquad: the shared DF2T core plus archet's own coefficient
/// derivation.
///
/// The derivation stays here rather than moving into `dsp::filters` because it
/// is not the codebase's: `2.0 * PI * (f0 / fs)` rounds differently from the
/// `2.0 * PI * f0 / fs` used elsewhere (35% of inputs), and the Q floors below
/// (0.2 / 0.3 / 0.05) are archet's alone. Every body model here is voiced
/// against exactly these numbers.
#[derive(Debug, Clone, Default)]
pub struct Biquad(phonix_dsp::filters::BiquadT);

impl Biquad {
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        self.0.process(x)
    }
    pub fn reset(&mut self) {
        self.0.reset();
    }
    fn coeffs(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self(phonix_dsp::filters::BiquadT::from_coeffs(b0, b1, b2, a0, a1, a2))
    }
    /// RBJ peaking EQ (boost/cut `gain_db` at `f0`).
    pub fn peaking(fs: f32, f0: f32, q: f32, gain_db: f32) -> Self {
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * (f0 / fs);
        let cw = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.2));
        Self::coeffs(1.0 + alpha * a, -2.0 * cw, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cw, 1.0 - alpha / a)
    }
    pub fn bandpass(fs: f32, f0: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * (f0 / fs);
        let cw = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.3));
        Self::coeffs(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
    }
    pub fn lowpass(fs: f32, f0: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * (f0 / fs);
        let cw = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.05));
        let b1 = 1.0 - cw;
        Self::coeffs(b1 * 0.5, b1, b1 * 0.5, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
    }
    pub fn highpass(fs: f32, f0: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * (f0 / fs);
        let cw = w0.cos();
        let alpha = w0.sin() / (2.0 * q.max(0.05));
        let b1 = -(1.0 + cw);
        Self::coeffs((1.0 + cw) * 0.5, b1, (1.0 + cw) * 0.5, 1.0 + alpha, -2.0 * cw, 1.0 - alpha)
    }
}

/// DENSE modal body: a parallel bank of resonators. A few EQ peaks read as a reed /
/// accordion -- a real violin body is DOZENS of overlapping modes with only moderate
/// Q (~25-50), and above ~1 kHz it is a STATISTICAL jagged field, not isolated peaks
/// (Gough 2016, Woodhouse 2014, Bissinger; the "wooden" timbre IS that density). So:
///   (a) ~9 discrete SIGNATURE modes < ~1.3 kHz (A0/CBR/A1/B1-/B1+ ...), strong
///       B1+/B1-/A0, weak CBR/A1, a few-dB dip through 650-1300 Hz (Dünnwald anti-
///       nasality -- the single biggest anti-reed lever);
///   (b) ~40 STATISTICAL modes 1.3-11 kHz, equally spaced ~170 Hz then JITTERED
///       (deterministic) so the response is jagged not comb-like, Q 35-55, gains
///       jittered, valleys only ~12-15 dB deep (overlapping skirts), shaped by the
///       broad BRIDGE HILL (~2.4 kHz) and a -12 dB/oct roll-off above ~3 kHz.
/// Each instrument has its own pattern, from its own measured modes (see the
/// specs below): a bass is not a violin scaled down, its air resonance sits
/// at a quarter of the violin's and its bridge hill below a kilohertz.
/// Output = parallel SUM of the resonators (the modal admittance), not a series EQ.

// Gains CALIBRATED to Dünnwald's old-Italian profile via the `dunnwald` test below:
// the A band (190-650, sonority) must be STRONG, the 650-1300 band suppressed
// (anti-nasality +4..6 dB), brilliance balanced with A, >4200 Hz well down (clarity).
const SIG_VIOLIN: &[(f32, f32, f32)] = &[
    (275.0, 10.0, 6.0),   // A0  main air (monopole)
    (405.0, 14.0, 4.0),   // CBR
    (460.0, 16.0, 2.0),   // B1-
    (485.0, 25.0, -8.0),  // A1
    (550.0, 18.0, 4.0),   // B1+
    (640.0, 20.0, -6.0),  // transitional
    (820.0, 12.0, -3.0),  // 650-1300 nasality dip
    (1010.0, 12.0, -3.0), // dip
    (1220.0, 15.0, 4.0),  // rising out of the dip
];

/// The statistical bank above the signature modes: the modes' Q (the
/// damping of a violin body's higher modes, Bissinger), and the modal
/// overlap they are laid out at, the ratio of a mode's half-power width to
/// the spacing, which exceeds one above about a kilohertz on a violin
/// (Woodhouse, The acoustics of the violin: a review, 2014).
const BANK_Q: f32 = 45.0;
const BANK_OVERLAP: f32 = 1.5;

/// One instrument's body: its measured signature modes, and the envelope the
/// statistical bank above them is built against. Nothing here is a scaled
/// violin. Frequencies are the published measurements cited on each table;
/// levels, the hill and the roll-offs are calibrated against the spectral
/// envelope of the same anechoic recordings the strings were calibrated on.
struct BodySpec {
    sig: &'static [(f32, f32, f32)], // (Hz, Q, dB)
    bank_from: f32,
    bank_to: f32,
    level_spacing: f32, // the spacing bank_db was calibrated at
    bank_db: f32,
    hill_f: f32,
    hill_gain: f32, // on the patch's bridge-hill dB
    roll_f: f32,
    roll_db_oct: f32,
    roll2_f: f32,
    roll2_db_oct: f32,
    hp_f: f32,
    lp_f: f32,
}

const VIOLIN: BodySpec = BodySpec {
    sig: SIG_VIOLIN,
    bank_from: 1320.0,
    bank_to: 11000.0,
    level_spacing: 170.0,
    bank_db: -9.0,
    hill_f: 2800.0,
    hill_gain: 0.5,
    roll_f: 3600.0,
    roll_db_oct: -10.0,
    roll2_f: 4200.0,
    roll2_db_oct: -16.0,
    hp_f: 320.0,
    lp_f: 4200.0,
};

// Viola: A0 measured at 208-244 Hz across five violas (Coffey 2013) and 224 Hz
// on a sixteen-inch instrument whose next modes sit at 328, 560, 1078 and
// 1504 Hz (Powell, UIUC REU); A1 at 390 Hz (Coffey); B1- and B1+ typically
// near 350 and 440 Hz. A viola radiates most between 450 and 1000 Hz
// (Leccese 2018), where a scaled violin puts its nasal dip.
const SIG_VIOLA: &[(f32, f32, f32)] = &[
    (224.0, 38.0, -14.0), // A0
    (305.0, 40.0, -18.0), // CBR, unmeasured, weak
    (350.0, 36.0, -12.0), // B1-
    (390.0, 40.0, -12.0), // A1
    (450.0, 42.0, -2.0),  // B1+
    (560.0, 30.0, 4.0),   // the strong mode above B1+
    (720.0, 24.0, -2.0),
    (950.0, 22.0, 0.0),   // air modes near 950-995 Hz
    (1078.0, 20.0, 0.0),
    (1504.0, 20.0, 2.0),
];
const VIOLA: BodySpec = BodySpec {
    sig: SIG_VIOLA,
    bank_from: 1300.0,
    bank_to: 11000.0,
    level_spacing: 150.0,
    bank_db: -2.0,
    hill_f: 1900.0,
    hill_gain: 0.4,
    roll_f: 2000.0,
    roll_db_oct: -14.0,
    roll2_f: 2400.0,
    roll2_db_oct: -16.0,
    hp_f: 300.0,
    lp_f: 3700.0,
};

// Cello: A0 90-104 Hz, Q about 17; B1- (T1) 144-168 Hz, Q 23-37; CBR about
// 170; C4 195; A1 203; B1+ 219; A3 277; A2 302 Hz (Bynum and Rossing, The
// Science of String Instruments ch. 14, table 14.2; Firth, STL-QPSR 1974).
// The bridge hill lies between 1 and 2.5 kHz and is less prominent than the
// violin's (Askenfelt, ch. 15; Woodhouse, Euphonics 5.3), the radiated
// formant at 800-1000 Hz (Rossing, ch. 14).
const SIG_CELLO: &[(f32, f32, f32)] = &[
    (100.0, 20.0, 3.0),   // A0
    (155.0, 30.0, 3.0),   // B1- (T1)
    (170.0, 40.0, -12.0), // CBR
    (195.0, 40.0, -6.0),  // C4
    (203.0, 30.0, -8.0),  // A1
    (219.0, 42.0, 4.0),   // B1+
    (277.0, 25.0, 8.0),   // A3
    (302.0, 25.0, 8.0),   // A2
    (400.0, 24.0, 8.0),
    (550.0, 22.0, 6.0),
    (900.0, 20.0, -4.0),  // the formant
    (1100.0, 20.0, -5.0),
];
const CELLO: BodySpec = BodySpec {
    sig: SIG_CELLO,
    bank_from: 1200.0,
    bank_to: 8000.0,
    level_spacing: 120.0,
    bank_db: -8.0,
    hill_f: 1500.0,
    hill_gain: 0.5,
    roll_f: 1800.0,
    roll_db_oct: -12.0,
    roll2_f: 2600.0,
    roll2_db_oct: -14.0,
    hp_f: 40.0,
    lp_f: 2500.0,
};

// Double bass: A0 58-68 Hz, T1 (B1-) 82-114 Hz on four conventional basses,
// plate modes equally spaced from 150 to 400 Hz, the lowest bridge resonance
// near 400 Hz, a maximum near 600 Hz and a rapid roll-off above it; the
// bridge hill at 500-1000 Hz (Askenfelt, Eigenmodes and tone quality of the
// double bass, STL-QPSR 1982; Askenfelt, The Science of String Instruments
// ch. 15; Fletcher and Rossing sec. 10.11). Played, a bass radiates most
// below 120 Hz (Leccese 2018).
const SIG_BASS: &[(f32, f32, f32)] = &[
    (65.0, 20.0, 0.0),    // A0
    (100.0, 28.0, 8.0),   // T1 (B1-), near the open G, the maximum of the radiated sound
    (130.0, 30.0, -10.0), // C4
    (150.0, 25.0, -8.0),  // plate modes, equally spaced
    (200.0, 25.0, 6.0),
    (250.0, 25.0, 5.0),
    (300.0, 25.0, -9.0),
    (400.0, 22.0, -11.0), // lowest bridge resonance
    (600.0, 20.0, -16.0), // the maximum before the roll-off
];
const BASS: BodySpec = BodySpec {
    sig: SIG_BASS,
    bank_from: 700.0,
    bank_to: 2500.0,
    level_spacing: 80.0,
    bank_db: -20.0,
    hill_f: 700.0,
    hill_gain: 0.2,
    roll_f: 800.0,
    roll_db_oct: -18.0,
    roll2_f: 1200.0,
    roll2_db_oct: -12.0,
    hp_f: 40.0,
    lp_f: 1500.0,
};

fn spec(inst: usize) -> &'static BodySpec {
    match inst {
        1 => &VIOLA,
        2 => &CELLO,
        3 => &BASS,
        _ => &VIOLIN,
    }
}

/// The shape the statistical bank is built against, in dB at one frequency:
/// the bridge hill, the two roll-offs above it, and the band limits the body
/// radiates between. The per-mode jitter is per-voice and deliberately not
/// here -- this is the envelope, which is what the ear hears as the body's
/// colour and what an editor should draw.
///
/// `inst` is the body index (0 violin .. 3 bass), the same one `new` takes.
pub fn envelope_db(inst: usize, bridge_hill_db: f32, hz: f32) -> f32 {
    let s = spec(inst);
    let hill_db = bridge_hill_db.max(5.0) * s.hill_gain;
    let fc = hz.max(1.0);
    let hump = hill_db * (-((fc / s.hill_f).log2().powi(2)) / 0.5).exp();
    let roll = if fc > s.roll_f { s.roll_db_oct * (fc / s.roll_f).log2() } else { 0.0 };
    let roll2 = if fc > s.roll2_f { s.roll2_db_oct * (fc / s.roll2_f).log2() } else { 0.0 };
    // The radiation high-pass below the lowest mode, and the output roll-off
    // above the brilliance band: both one-pole-ish, drawn as such.
    let hp_db = -10.0 * (1.0 + (s.hp_f / fc).powi(4)).log10();
    let lp_db = -10.0 * (1.0 + (fc / s.lp_f).powi(4)).log10();
    s.bank_db + hump + roll + roll2 + hp_db + lp_db
}

#[derive(Debug, Clone)]
pub struct ModalBody {
    hp: Biquad,
    res: Vec<(Biquad, f32)>, // parallel resonators (bandpass) and their gains
    lp: Biquad, // output roll-off: the F band (4200-6879) rides the mode SKIRTS, a
    // per-mode gain roll can't reach it -- Dünnwald clarity needs this cut.
    norm: f32,
}

impl ModalBody {
    /// `inst` body index (0 violin..3 bass); `detune` per-voice freq multiplier;
    /// `bridge_hill_db` the bridge-hill presence (the brilliance/projection formant).
    pub fn new(fs: f32, inst: usize, detune: f32, bridge_hill_db: f32) -> Self {
        let s = spec(inst);
        let hill_f = s.hill_f * detune;
        let hill_db = bridge_hill_db.max(5.0) * s.hill_gain;
        let nyq = fs * 0.45;
        let mut res: Vec<(Biquad, f32)> = Vec::new();
        // (a) the instrument's own signature modes, measured, detuned per
        // voice so no two players share a body.
        for &(f, q, g) in s.sig {
            let fc = (f * detune).clamp(25.0, nyq);
            res.push((Biquad::bandpass(fs, fc, q), 10f32.powf(g / 20.0)));
        }
        // (b) statistical bank: dense, deterministically jittered. The seed is PER-VOICE
        // (folds in `detune`, which is unique per voice) so each player's body is its own
        // irregular shape -- otherwise every violin gets the IDENTICAL jagged response and
        // they reinforce into one static buzz = a bank of accordion reeds.
        let mut seed: u32 = 0x9E37_79B9
            ^ ((inst as u32 + 1).wrapping_mul(2654435761))
            ^ detune.to_bits();
        let mut rng = || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
                           seed as f32 / u32::MAX as f32 };
        let mut f = s.bank_from * detune;
        let roll_f = s.roll_f * detune;
        let roll2_f = s.roll2_f * detune;
        while f < nyq && f < s.bank_to {
            // Modes as dense as their own width times the overlap a body
            // shows above its signature modes, so the response between
            // them fluctuates by a few decibels rather than falling into a
            // notch; the spacing therefore grows with frequency.
            let spacing = f / BANK_Q / BANK_OVERLAP;
            let fc = (f + (rng() - 0.5) * spacing * 0.8).clamp(25.0, nyq);
            // envelope: the bridge hill, a roll-off above it and an extra roll
            // higher still (on the violin, Duennwald's clarity: the band above
            // the brilliance sits well under it), plus per-voice jitter. The
            // three terms are `envelope_db`'s, minus the band limits it draws
            // and the jitter it cannot.
            let hump = hill_db * (-((fc / hill_f).log2().powi(2)) / 0.5).exp();
            let roll = if fc > roll_f { s.roll_db_oct * (fc / roll_f).log2() } else { 0.0 };
            let roll2 = if fc > roll2_f { s.roll2_db_oct * (fc / roll2_f).log2() } else { 0.0 };
            // Overlapping modes add, so each mode's power is scaled by the
            // density relative to the spacing the level was calibrated at.
            let share_db = -10.0 * (s.level_spacing / spacing).max(1.0).log10();
            let g_db = s.bank_db + hump + roll + roll2 + share_db + (rng() - 0.5) * 8.0;
            // Q: violin-family modes RING. They are real corpus modes, masked
            // under the bow's continuous excitation.
            let q = BANK_Q * (0.8 + rng() * 0.4);
            res.push((Biquad::bandpass(fs, fc, q), 10f32.powf(g_db / 20.0)));
            f += spacing;
        }
        // radiation high-pass below the lowest mode (the body cannot radiate
        // there), and the output roll-off above the brilliance band
        let hp = Biquad::highpass(fs, (s.hp_f * detune).clamp(30.0, 300.0), 0.7);
        let lp_f = (s.lp_f * detune).clamp(600.0, 9000.0);
        let lp = Biquad::lowpass(fs, lp_f, 0.7);
        // normalize so the summed bank sits at a sane level
        let gsum: f32 = res.iter().map(|(_, g)| *g).sum::<f32>().max(1e-3);
        Self { hp, res, lp, norm: 30.0 / gsum.sqrt() }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let xin = self.hp.process(x);
        let mut y = 0.0f32;
        for (bp, g) in self.res.iter_mut() {
            y += bp.process(xin) * *g;
        }
        self.lp.process(y * self.norm)
    }

    pub fn reset(&mut self) {
        self.hp.reset();
        self.lp.reset();
        for (bp, _) in self.res.iter_mut() {
            bp.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dünnwald band profile of the violin body (impulse response -> band levels).
    /// Old-Italian targets (Dünnwald 1991 via Buen): A(190-650) strong; B(650-1300)
    /// suppressed ~4-6 dB below A (anti-nasality, his strongest discriminator);
    /// DE(1640-4200) within ~3 dB of A (brilliance / bridge hill);
    /// clarity = DE - F(4200-6879) >= 10 dB (no harshness).
    ///   cargo test --release --lib body::tests::dunnwald -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic"]
    fn dunnwald() {
        let fs = 48_000.0f32;
        let mut body = ModalBody::new(fs, 0, 1.0, 9.0);
        // impulse response (2 s is plenty at Q<=55)
        let n = 1 << 17;
        let mut h = vec![0.0f32; n];
        for (i, y) in h.iter_mut().enumerate() {
            *y = body.process(if i == 0 { 1.0 } else { 0.0 });
        }
        // naive DFT band powers (coarse 10 Hz grid is fine for band integrals)
        let band = |lo: f32, hi: f32| -> f32 {
            let mut p = 0.0f64;
            let mut f = lo;
            while f < hi {
                let w = 2.0 * std::f64::consts::PI * f as f64 / fs as f64;
                let (mut re, mut im) = (0.0f64, 0.0f64);
                for (i, &x) in h.iter().enumerate() {
                    re += x as f64 * (w * i as f64).cos();
                    im -= x as f64 * (w * i as f64).sin();
                }
                p += re * re + im * im;
                f += 10.0;
            }
            10.0 * (p / ((hi - lo) as f64 / 10.0)).log10() as f32
        };
        let a = band(190.0, 650.0);
        let b = band(650.0, 1300.0);
        let c = band(1300.0, 2580.0);
        let de = band(1640.0, 4200.0);
        let f_ = band(4200.0, 6879.0);
        println!("DÜNNWALD bands (dB): A(190-650)={:.1} B(650-1300)={:.1} C(1300-2580)={:.1} DE(1640-4200)={:.1} F(4200-6879)={:.1}", a, b, c, de, f_);
        println!("  anti-nasality A-B = {:+.1} dB (want +4..+6)", a - b);
        println!("  brilliance  DE-A = {:+.1} dB (want > -3)", de - a);
        println!("  clarity     DE-F = {:+.1} dB (want >= +10)", de - f_);
        // The band integrals say nothing about TIME: a bank at Q 35 and one at
        // Q 500 can share them exactly. What decides whether the body knocks
        // like wood or drones is how fast its own impulse response dies, and
        // how much of its energy lands in the first few milliseconds. Measured
        // on the envelope, with no filter in the path: a band-limited decay
        // read through a brick wall measures the wall.
        let hop = (0.001 * fs) as usize;
        let env: Vec<f32> = h
            .chunks(hop)
            .map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt())
            .collect();
        let epk = env.iter().cloned().fold(0.0f32, f32::max).max(1e-12);
        let at = |db: f32| -> f32 {
            env.iter()
                .position(|&e| 20.0 * (e / epk).log10() <= db)
                .map(|i| i as f32)
                .unwrap_or(f32::NAN)
        };
        println!("  ring: -20 dB at {:.0} ms, -40 dB at {:.0} ms", at(-20.0), at(-40.0));
        let sq = |s: &[f32]| -> f64 { s.iter().map(|x| (*x as f64) * (*x as f64)).sum() };
        let total = sq(&h).max(1e-30);
        for ms in [2.0f32, 10.0, 50.0, 200.0] {
            let n = ((ms / 1000.0 * fs) as usize).min(h.len());
            println!("  energy in the first {ms:>5.0} ms: {:>5.1} %", 100.0 * sq(&h[..n]) / total);
        }
        // The response itself, as raw little-endian f32, so the loss a string
        // sees through this body can be read off it at any partial.
        let bytes: Vec<u8> = h.iter().flat_map(|x| x.to_le_bytes()).collect();
        std::fs::write("/tmp/archet_body_ir.f32", bytes).expect("write the impulse response");
        println!("  wrote /tmp/archet_body_ir.f32 ({} samples at {fs} Hz)", h.len());
        // And every instrument's body, so each can be held against the
        // spectral envelope its recordings have.
        for inst in 1..4usize {
            let mut body = ModalBody::new(fs, inst, 1.0, 9.0);
            let h: Vec<f32> = (0..n).map(|i| body.process(if i == 0 { 1.0 } else { 0.0 })).collect();
            let bytes: Vec<u8> = h.iter().flat_map(|x| x.to_le_bytes()).collect();
            std::fs::write(format!("/tmp/archet_body_ir_{inst}.f32"), bytes).expect("write");
        }
        println!("  wrote /tmp/archet_body_ir_{{1,2,3}}.f32 (viola, cello, bass)");
    }
}

#[cfg(test)]
mod shared_filter_migration {
    use super::*;
    fn probe() -> Vec<f32> {
        let mut sig = Vec::new(); let mut st: u32 = 12345;
        for i in 0..512 {
            st = st.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (st >> 8) as f32 / 8388608.0 - 1.0;
            let s = (i as f32 * 0.07).sin();
            sig.push(if i == 0 { 1.0 } else { 0.6 * n + 0.4 * s });
        }
        sig
    }
    fn hash(o: &[f32]) -> u64 { o.iter().fold(0u64, |a, &v| a.rotate_left(7) ^ v.to_bits() as u64) }
    /// The body EQ moved onto the shared DF2T core. These hashes were captured
    /// from the hand-rolled biquad before the move: archet's own w0 rounding and
    /// Q floors must survive it, or every body model is re-voiced.
    #[test]
    fn body_biquad_survived_the_dsp_filters_migration() { 
        let sig = probe();
        for (name, mk, golden) in [
            ("peaking",  0u8, 0x77ccd2718bb70ea3u64),
            ("bandpass", 1,   0xfc646c28ab050bec),
            ("lowpass",  2,   0xc0b8322af52bbedd),
            ("highpass", 3,   0xe53b3c0d10259f58),
        ] {
            let mut f = match mk {
                0 => Biquad::peaking(44100.0, 1000.0, 1.2, 6.0),
                1 => Biquad::bandpass(44100.0, 800.0, 2.0),
                2 => Biquad::lowpass(44100.0, 3000.0, 0.707),
                _ => Biquad::highpass(44100.0, 300.0, 0.707),
            };
            let o: Vec<f32> = sig.iter().map(|&x| f.process(x)).collect();
            assert_eq!(hash(&o), golden, "archet {name} a derive");
            println!("ARCHET {name} OK");
        }
 }

    /// Full ModalBody (dense resonator bank) byte-stability lock so a future
    /// refactor of the hot per-sample resonator loop is provably output-
    /// preserving (cf archet::modal::tests::process_is_byte_stable). Covers the
    /// constructor's deterministic mode placement AND process(). detune = 1.0
    /// seeds the statistical-bank jitter deterministically.
    #[test]
    fn modal_body_process_is_byte_stable() {
        let sig = probe();
        let mut body = ModalBody::new(48_000.0, 0, 1.0, 6.0); // violin body
        let o: Vec<f32> = sig.iter().map(|&x| body.process(x)).collect();
        assert_eq!(hash(&o), 8651240168125589848u64, "archet ModalBody drifted");
    }
}
