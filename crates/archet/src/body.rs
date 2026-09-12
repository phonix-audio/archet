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
/// Whole pattern scales down with instrument size (viola .80, cello .45, bass .33).
/// Output = parallel SUM of the resonators (the modal admittance), not a series EQ.

// Gains CALIBRATED to Dünnwald's old-Italian profile via the `dunnwald` test below:
// the A band (190-650, sonority) must be STRONG, the 650-1300 band suppressed
// (anti-nasality +4..6 dB), brilliance balanced with A, >4200 Hz well down (clarity).
const SIG_VIOLIN: &[(f32, f32, f32)] = &[
    (270.0, 38.0, 1.0),   // A0  main air (monopole)
    (405.0, 40.0, -12.0), // CBR weak radiator
    (460.0, 36.0, 3.0),   // B1- strong
    (485.0, 40.0, -8.0),  // A1
    (550.0, 42.0, 4.0),   // B1+ strongest
    (640.0, 28.0, -2.0),  // transitional
    (820.0, 24.0, -11.0), // 650-1300 nasality dip
    (1010.0, 22.0, -12.0),// dip
    (1220.0, 20.0, -8.0), // rising out of the dip
];

/// The shape the statistical bank is built against, in dB at one frequency:
/// the bridge hill, the two roll-offs above it, and the band limits the body
/// radiates between. The per-mode jitter is per-voice and deliberately not
/// here -- this is the envelope, which is what the ear hears as the body's
/// colour and what an editor should draw.
///
/// `inst` is the body index (0 violin .. 3 bass), the same one `new` takes.
pub fn envelope_db(inst: usize, bridge_hill_db: f32, hz: f32) -> f32 {
    let scale = match inst { 1 => 0.80, 2 => 0.45, 3 => 0.33, _ => 1.0 };
    let hill_f = 2400.0 * scale;
    let hill_db = bridge_hill_db.max(5.0) * 0.8;
    let fc = hz.max(1.0);
    let hump = hill_db * (-((fc / hill_f).log2().powi(2)) / 0.5).exp();
    let roll = if fc > 3000.0 * scale { -12.0 * (fc / (3000.0 * scale)).log2() } else { 0.0 };
    let roll2 = if fc > 4200.0 * scale { -10.0 * (fc / (4200.0 * scale)).log2() } else { 0.0 };
    // The radiation high-pass below the lowest mode, and the output roll-off
    // above the brilliance band: both one-pole-ish, drawn as such.
    let hp_f = (200.0 * scale).clamp(30.0, 300.0);
    let lp_f = (4600.0 * scale).clamp(1500.0, 9000.0);
    let hp_db = -10.0 * (1.0 + (hp_f / fc).powi(4)).log10();
    let lp_db = -10.0 * (1.0 + (fc / lp_f).powi(4)).log10();
    -4.0 + hump + roll + roll2 + hp_db + lp_db
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
        let scale = match inst { 1 => 0.80, 2 => 0.45, 3 => 0.33, _ => 1.0 } * detune;
        let hill_f = 2400.0 * scale;
        let hill_db = bridge_hill_db.max(5.0) * 0.8;
        let nyq = fs * 0.45;
        let mut res: Vec<(Biquad, f32)> = Vec::new();
        // (a) signature modes: the violin corpus formants, scaled to the
        // instrument. Every body here is a member of that family.
        for &(f, q, g) in SIG_VIOLIN {
            let fc = (f * scale).clamp(25.0, nyq);
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
        let spacing = 170.0 * scale;
        let mut f = 1320.0 * scale;
        while f < nyq && f < 11000.0 {
            let fc = (f + (rng() - 0.5) * spacing * 0.8).clamp(25.0, nyq);
            // envelope: broad bridge hill bump + roll-off above ~3k*scale + an EXTRA
            // roll above ~4.2k*scale (Dünnwald clarity: the 4200-6879 band of a fine
            // violin sits >=10 dB under the brilliance band -- no harshness) + jitter.
            // The three terms are `envelope_db`'s, minus the band limits it draws
            // and the per-voice jitter it cannot.
            let hump = hill_db * (-((fc / hill_f).log2().powi(2)) / 0.5).exp();
            let roll = if fc > 3000.0 * scale { -12.0 * (fc / (3000.0 * scale)).log2() } else { 0.0 };
            let roll2 = if fc > 4200.0 * scale { -10.0 * (fc / (4200.0 * scale)).log2() } else { 0.0 };
            let g_db = -4.0 + hump + roll + roll2 + (rng() - 0.5) * 8.0;
            // Q: violin-family modes RING. They are real corpus modes, masked
            // under the bow's continuous excitation.
            let q = 35.0 + rng() * 20.0;
            res.push((Biquad::bandpass(fs, fc, q), 10f32.powf(g_db / 20.0)));
            f += spacing;
        }
        // sub-A0 radiation high-pass (the body can't radiate below its lowest mode)
        let hp = Biquad::highpass(fs, (200.0 * scale).clamp(30.0, 300.0), 0.7);
        let lp_f = (4600.0 * scale).clamp(1500.0, 9000.0);
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
        assert_eq!(hash(&o), 5182835411810185102u64, "archet ModalBody drifted");
    }
}
