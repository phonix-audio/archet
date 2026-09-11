//! FOF choir synthesis — Fonctions d'Onde Formantique (Rodet / IRCAM
//! CHANT, 1984). The documented technique for convincing synthetic
//! singing voice / choir.
//!
//! Each vowel is built from several formants. Each formant is a stream
//! of *grains*: a sine at the formant centre frequency windowed by a
//! raised-cosine attack + exponential decay (the decay rate sets the
//! formant bandwidth). A new grain is triggered every glottal period
//! (1/f0); grains overlap-add. A real choir = several such voices, each
//! slightly detuned with its own vibrato, jitter (period wobble),
//! shimmer (amplitude wobble) and vocal-tract scaling.
//!
//! Performance: `vowel_formants()` (4 powf) and the window cosines are
//! the per-sample hot spots, so the formant set is cached (recomputed
//! only when vowel/brightness change) and the windows use a fast sine
//! approximation. Singer count is runtime-variable (1..MAX_SINGERS).

pub const MAX_SINGERS: usize = 16;
/// Bandwidth of each singer's intonation walk. Slow enough to read as a
/// section breathing rather than as a modulation.
const DRIFT_HZ: f32 = 0.4;
/// The walk is stepped at a rate fixed in time, not once a sample. Stepped
/// per sample it consumes its randomness twice as fast at twice the rate
/// and takes a different path, so the same note renders to a different
/// level at every sample rate.
const DRIFT_UPDATE_HZ: f32 = 1000.0;
/// The count the spread is written for. A choir of this many is what every
/// constant here was set against, and stays untouched as the count moves.
const REFERENCE_SINGERS: usize = 4;
/// The corner the aspiration is tilted at, and the rate its density is
/// written for. White noise carries a fixed variance per sample, so the
/// same generator spread over twice the bandwidth is half as dense in the
/// band a listener hears; both are held in hertz so the air sounds the
/// same at every rate.
const BREATH_TILT_HZ: f32 = 3300.0;
const REFERENCE_RATE: f32 = 48_000.0;
/// Where the choir's output is cleared of its own offset. A grain is a sine
/// cut short: its mean is only zero when its length covers many periods, so
/// a low first formant leaves one behind, and every singer leaves the same
/// one. They add in phase where the voices do not.
const DC_BLOCK_HZ: f32 = 12.0;
const NUM_FORMANTS: usize = 4;
const MAX_GRAINS: usize = 8;
const TAU: f32 = std::f32::consts::TAU;
const PI: f32 = std::f32::consts::PI;
const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;

#[inline(always)]
fn fast_sin(x: f32) -> f32 {
    // Parabolic sine approximation, input in radians.
    let mut p = x * (1.0 / TAU);
    p -= p.floor();
    let q = p - 0.5;
    -16.0 * q * (0.5 - q.abs())
}

/// `0.5 * (1 - cos(pi*x))` for x in 0..1, via the fast sine (cos = sin+90deg).
#[inline(always)]
fn rcos_window(x: f32) -> f32 {
    0.5 * (1.0 - fast_sin(PI * x + HALF_PI))
}

// ── Vowel formant tables (alto choir): freq Hz, bandwidth Hz, amp dB ──
const VOWELS: usize = 5;
const F_FREQ: [[f32; NUM_FORMANTS]; VOWELS] = [
    [ 800.0, 1150.0, 2800.0, 3500.0], // Ah
    [ 400.0, 1600.0, 2700.0, 3300.0], // Eh
    [ 350.0, 1700.0, 2700.0, 3700.0], // Ee
    [ 450.0,  800.0, 2830.0, 3500.0], // Oh
    [ 325.0,  700.0, 2530.0, 3500.0], // Oo
];
const F_BW: [[f32; NUM_FORMANTS]; VOWELS] = [
    [80.0,  90.0, 120.0, 130.0],
    [60.0,  80.0, 120.0, 150.0],
    [50.0, 100.0, 120.0, 150.0],
    [70.0,  80.0, 100.0, 130.0],
    [50.0,  60.0, 170.0, 180.0],
];
const F_DB: [[f32; NUM_FORMANTS]; VOWELS] = [
    [0.0,  -4.0, -20.0, -36.0],
    [0.0, -24.0, -30.0, -35.0],
    [0.0, -20.0, -30.0, -36.0],
    [0.0,  -9.0, -16.0, -28.0],
    [0.0, -12.0, -30.0, -40.0],
];

#[inline]
fn db_to_lin(db: f32) -> f32 { 10.0f32.powf(db / 20.0) }

/// What the choir is asked to sing.
#[derive(Clone, Copy, Debug)]
pub struct Voicing {
    /// 0 (ah) through eh, ee, oh to 1 (oo).
    pub vowel: f32,
    /// Pitch ratio of each singer's vibrato, around 0.01 to 0.03.
    pub vib_depth: f32,
    /// Aspiration, 0..1.
    pub breath: f32,
    /// Upper-formant lift, 0..1 with 0.5 neutral.
    pub brightness: f32,
    pub singers: usize,
    /// How much of the vowel table's dB range reaches the grains. Below
    /// about 0.7 the first two formants merge into one hump and the vowel
    /// stops being a vowel.
    pub contrast: f32,
    /// Standard deviation of each singer's own slow intonation walk, in
    /// cents. This is what spreads a harmonic into a band; a static detune
    /// spread does not.
    pub drift_cents: f32,
    /// Send the aspiration through the vowel's formants, so it leaves as a
    /// breathy vowel rather than as flat noise.
    pub breath_shaped: bool,
}

impl Default for Voicing {
    fn default() -> Self {
        Voicing {
            vowel: 0.0,
            vib_depth: 0.02,
            breath: 0.3,
            brightness: 0.5,
            singers: 4,
            contrast: 0.5,
            drift_cents: 0.0,
            breath_shaped: false,
        }
    }
}

/// A two-pole resonator, normalised to unit gain at its centre frequency
/// so a parallel bank sums to the amplitudes the vowel table asks for.
#[derive(Clone, Copy, Default)]
struct Resonator {
    a: f32,
    b: f32,
    c: f32,
    y1: f32,
    y2: f32,
}

impl Resonator {
    fn set(&mut self, f: f32, bw: f32, sr: f32) {
        let f = f.clamp(20.0, sr * 0.48);
        let r = (-PI * bw / sr).exp();
        let theta = TAU * f / sr;
        self.b = 2.0 * r * theta.cos();
        self.c = -r * r;
        let (s1, c1) = theta.sin_cos();
        let (s2, c2) = (2.0 * theta).sin_cos();
        let re = 1.0 - self.b * c1 - self.c * c2;
        let im = self.b * s1 + self.c * s2;
        self.a = (re * re + im * im).sqrt();
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.a * x + self.b * self.y1 + self.c * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Interpolated, choir-widened formant set for vowel 0..1, with a
/// brightness control (0.5 = neutral) lifting the upper formants and a
/// contrast control setting how much of the table's dB range survives.
fn vowel_formants(v: f32, brightness: f32, contrast: f32) -> [(f32, f32, f32); NUM_FORMANTS] {
    let pos = v.clamp(0.0, 1.0) * (VOWELS - 1) as f32;
    let a = (pos as usize).min(VOWELS - 2);
    let b = a + 1;
    let t = pos - a as f32;
    let up = 0.4 + brightness.clamp(0.0, 1.0) * 1.2; // upper-formant gain
    let mut out = [(0.0, 0.0, 0.0); NUM_FORMANTS];
    for f in 0..NUM_FORMANTS {
        let freq = F_FREQ[a][f] * (1.0 - t) + F_FREQ[b][f] * t;
        // Choir = many singers -> effective formants far wider than a
        // single voice, so 2-3 harmonics pass under each.
        let bw = (F_BW[a][f] * (1.0 - t) + F_BW[b][f] * t) * 2.6;
        let mut amp = db_to_lin((F_DB[a][f] * (1.0 - t) + F_DB[b][f] * t) * contrast.clamp(0.0, 1.0));
        if f >= 1 { amp *= up; }
        out[f] = (freq, bw, amp);
    }
    out
}

#[derive(Clone, Copy, Default)]
struct Grain {
    active: bool,
    phase: f32,
    phase_inc: f32,
    env: f32,
    decay: f32,
    amp: f32,
    age: u32,
    kris: u32,
    kdur: u32,
    kdec: u32,
}

impl Grain {
    fn trigger(&mut self, fc: f32, bw: f32, amp: f32, sr: f32) {
        self.active = true;
        self.phase = 0.0;
        self.phase_inc = TAU * fc / sr;
        self.env = 1.0;
        self.decay = (-PI * bw / sr).exp();
        self.amp = amp;
        self.age = 0;
        self.kris = (0.003 * sr) as u32;
        let dur_s = (4.0 / (PI * bw)).clamp(0.004, 0.022);
        self.kdur = (dur_s * sr) as u32;
        self.kdec = (0.003 * sr) as u32;
    }

    #[inline(always)]
    fn process(&mut self) -> f32 {
        if !self.active { return 0.0; }
        let s = fast_sin(self.phase);
        self.phase += self.phase_inc;
        if self.phase >= TAU { self.phase -= TAU; }

        let mut a = self.env;
        if self.age < self.kris {
            a *= rcos_window(self.age as f32 / self.kris.max(1) as f32);
        }
        if self.age + self.kdec > self.kdur {
            let rem = self.kdur.saturating_sub(self.age) as f32 / self.kdec.max(1) as f32;
            a *= rcos_window(rem);
        }
        self.env *= self.decay;
        self.age += 1;
        if self.age >= self.kdur { self.active = false; }
        s * a * self.amp
    }
}

#[derive(Clone)]
struct Singer {
    sr: f32,
    grains: [[Grain; MAX_GRAINS]; NUM_FORMANTS],
    rr: [usize; NUM_FORMANTS],
    samples_to_trigger: f32,
    detune: f32,
    formant_scale: f32,
    vib_phase: f32,
    vib_rate: f32,
    human_vib: crate::vibrato::HumanVibrato,
    pan_l: f32,
    pan_r: f32,
    rng: u32,
    jitter: f32,
    shimmer: f32,
    breath_lp: f32,
    breath_k: f32,
    breath_gain: f32,
    /// A one-pole low-pass of white noise: this singer's own slow walk
    /// around the written pitch, independent of every other singer's.
    drift: f32,
    drift_k: f32,
    /// Scales the walk so its standard deviation is the asked-for cents.
    drift_norm: f32,
    /// Samples between two steps of the walk, and the countdown to the next.
    drift_period: f32,
    drift_countdown: f32,
}

impl Singer {
    fn new(sr: f32, idx: usize) -> Self {
        let mut s = Self {
            sr,
            grains: [[Grain::default(); MAX_GRAINS]; NUM_FORMANTS],
            rr: [0; NUM_FORMANTS],
            samples_to_trigger: 0.0,
            detune: 1.0,
            formant_scale: 1.0,
            vib_phase: (idx as f32) * 0.37,
            vib_rate: 5.2,
            human_vib: crate::vibrato::HumanVibrato::default(),
            pan_l: 0.707,
            pan_r: 0.707,
            rng: 0x1234_5677u32.wrapping_add((idx as u32 + 1).wrapping_mul(2654435761)),
            jitter: 0.0,
            shimmer: 1.0,
            breath_lp: 0.0,
            breath_k: 0.0,
            breath_gain: 1.0,
            drift: 0.0,
            drift_k: 0.0,
            drift_norm: 0.0,
            drift_period: 1.0,
            drift_countdown: 0.0,
        };
        s.set_drift_rate(DRIFT_HZ, sr);
        s.breath_k = 1.0 - (-TAU * BREATH_TILT_HZ / sr).exp();
        s.breath_gain = (sr / REFERENCE_RATE).sqrt();
        s.configure(idx, 4);
        s
    }

    /// The walk's bandwidth, and the gain that puts its standard deviation
    /// on the asked-for cents. `next_rand` is uniform over [-1, 1], whose
    /// standard deviation is 1/sqrt(3); a one-pole low-pass of it scales
    /// that by sqrt(k / (2 - k)).
    fn set_drift_rate(&mut self, hz: f32, sr: f32) {
        let k = 1.0 - (-TAU * hz / DRIFT_UPDATE_HZ).exp();
        self.drift_k = k;
        self.drift_norm = 3.0f32.sqrt() * ((2.0 - k) / k).sqrt();
        self.drift_period = (sr / DRIFT_UPDATE_HZ).max(1.0);
    }

    /// Recompute the ensemble spread for this singer given the active count.
    ///
    /// The spread grows with the count so the spacing between neighbours
    /// stays what it is at four. Held to a fixed range instead, every
    /// singer added past the fourth lands within a couple of cents of the
    /// one beside it and sings the same thing: the section gets no wider,
    /// only more correlated, and the sum walks back toward the middle.
    fn configure(&mut self, idx: usize, count: usize) {
        let frac = if count > 1 { idx as f32 / (count - 1) as f32 * 2.0 - 1.0 } else { 0.0 };
        let pan = frac * 0.8;
        let p = (pan + 1.0) * 0.5 * HALF_PI;
        let growth = count as f32 / REFERENCE_SINGERS as f32;
        self.detune = 2.0f32.powf(frac * 0.09 * growth / 12.0);
        self.formant_scale = 1.0 + frac * 0.04 * growth.sqrt();
        self.vib_rate = 5.2 + frac * 0.8;
        self.pan_l = p.cos();
        self.pan_r = p.sin();
    }

    #[inline(always)]
    fn next_rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng & 0xFFFF) as f32 / 65535.0 * 2.0 - 1.0
    }

    /// A fresh note. Each singer's first grain lands at its own moment
    /// inside the first few milliseconds: singers that all started on the
    /// same sample summed coherently, then beat against each other into a
    /// dip a tenth of a second in.
    fn reset(&mut self) {
        for fo in &mut self.grains { for g in fo.iter_mut() { g.active = false; } }
        self.samples_to_trigger = self.next_rand().abs() * self.sr * 0.005;
        self.breath_lp = 0.0;
        self.drift = 0.0;
        self.drift_countdown = 0.0;
        self.vib_phase = self.next_rand().abs();
        self.human_vib = crate::vibrato::HumanVibrato::default();
        self.human_vib.vib_phase = self.vib_phase;
    }

    /// One stereo sample of voice, plus this singer's own aspiration noise
    /// left unshaped so the choir can send it through the vowel.
    #[inline]
    fn process(&mut self, f0: f32, v: &Voicing,
               formants: &[(f32, f32, f32); NUM_FORMANTS]) -> (f32, f32, f32) {
        // Human vibrato per singer (flutter + per-cycle wander + non-sinusoidal
        // shape) instead of a plain sine; output kept as a small pitch ratio to
        // match the former magnitude. rng decoupled into a local for the closure.
        let vib = {
            let mut r = self.rng;
            let (v, _s, _a) = self.human_vib.step(
                self.sr, 0.0, self.vib_rate, 1.0, v.vib_depth, 1.0, 1.0, 0.0, 0.0,
                || { r ^= r << 13; r ^= r >> 17; r ^= r << 5; (r & 0xFFFF) as f32 / 65535.0 * 2.0 - 1.0 },
            );
            self.rng = r;
            v
        };
        let drift = if v.drift_cents > 0.0 {
            self.drift_countdown -= 1.0;
            if self.drift_countdown <= 0.0 {
                self.drift_countdown += self.drift_period;
                self.drift += (self.next_rand() - self.drift) * self.drift_k;
            }
            crate::fastmath::fast_exp2(self.drift * self.drift_norm * v.drift_cents / 1200.0)
        } else {
            1.0
        };
        let f = f0 * self.detune * drift * (1.0 + vib + self.jitter * 0.01);

        if self.samples_to_trigger <= 0.0 {
            self.jitter = self.next_rand() * 0.3;
            self.shimmer = 1.0 + self.next_rand() * 0.12;
            for fi in 0..NUM_FORMANTS {
                let (fc, bw, amp) = formants[fi];
                let slot = self.rr[fi];
                self.grains[fi][slot].trigger(fc * self.formant_scale, bw, amp * self.shimmer, self.sr);
                self.rr[fi] = (slot + 1) % MAX_GRAINS;
            }
            let period = (self.sr / f.max(20.0)).max(1.0);
            self.samples_to_trigger += period;
        }
        self.samples_to_trigger -= 1.0;

        let mut mono = 0.0f32;
        for fi in 0..NUM_FORMANTS {
            for g in self.grains[fi].iter_mut() {
                mono += g.process();
            }
        }
        let breath = if v.breath > 0.0 {
            let n = self.next_rand() * self.breath_gain;
            self.breath_lp += self.breath_k * (n - self.breath_lp);
            n - self.breath_lp
        } else {
            0.0
        };
        (mono * self.pan_l, mono * self.pan_r, breath)
    }
}

/// A choir of `count` decorrelated FOF singers (1..MAX_SINGERS).
#[derive(Clone)]
pub struct FofChoir {
    singers: Vec<Singer>,
    count: usize,
    sr: f32,
    cached: [(f32, f32, f32); NUM_FORMANTS],
    c_vowel: f32,
    c_bright: f32,
    c_contrast: f32,
    c_valid: bool,
    /// One bank a side, tuned to the vowel being sung and fed the summed
    /// aspiration. Turbulence excites the tract broadly, so these sit wider
    /// than the grains that carry the voice.
    breath_l: [Resonator; NUM_FORMANTS],
    breath_r: [Resonator; NUM_FORMANTS],
    dc: [DcBlock; 2],
}

/// A one-pole, one-zero high pass that removes a constant and leaves the
/// rest where it was.
#[derive(Clone, Copy, Default)]
struct DcBlock {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlock {
    fn new(sr: f32) -> DcBlock {
        DcBlock { r: 1.0 - TAU * DC_BLOCK_HZ / sr, x1: 0.0, y1: 0.0 }
    }

    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
}

impl FofChoir {
    pub fn new(sr: f32) -> Self {
        Self {
            singers: (0..MAX_SINGERS).map(|i| Singer::new(sr, i)).collect(),
            count: 4,
            sr,
            cached: [(0.0, 0.0, 0.0); NUM_FORMANTS],
            c_vowel: -1.0,
            c_bright: -1.0,
            c_contrast: -1.0,
            c_valid: false,
            breath_l: [Resonator::default(); NUM_FORMANTS],
            breath_r: [Resonator::default(); NUM_FORMANTS],
            dc: [DcBlock::new(sr); 2],
        }
    }

    pub fn note_on(&mut self) {
        for s in &mut self.singers { s.reset(); }
        for r in &mut self.breath_l { r.reset(); }
        for r in &mut self.breath_r { r.reset(); }
        for d in &mut self.dc { d.reset(); }
    }

    fn set_count(&mut self, n: usize) {
        let n = n.clamp(1, MAX_SINGERS);
        if n != self.count {
            self.count = n;
            for i in 0..n { self.singers[i].configure(i, n); }
        }
    }

    /// Render one stereo sample.
    #[inline]
    pub fn process(&mut self, f0: f32, v: &Voicing) -> (f32, f32) {
        self.set_count(v.singers);
        // The formant set costs four powf, so it is recomputed only when
        // one of the three things it depends on moves.
        if !self.c_valid
            || (v.vowel - self.c_vowel).abs() > 1e-4
            || (v.brightness - self.c_bright).abs() > 1e-4
            || (v.contrast - self.c_contrast).abs() > 1e-4
        {
            self.cached = vowel_formants(v.vowel, v.brightness, v.contrast);
            self.c_vowel = v.vowel;
            self.c_bright = v.brightness;
            self.c_contrast = v.contrast;
            self.c_valid = true;
            if v.breath_shaped {
                for f in 0..NUM_FORMANTS {
                    let (fc, bw, _) = self.cached[f];
                    self.breath_l[f].set(fc, bw * 2.5, self.sr);
                    self.breath_r[f].set(fc, bw * 2.5, self.sr);
                }
            }
        }
        let mut l = 0.0f32;
        let mut r = 0.0f32;
        let mut bl = 0.0f32;
        let mut br = 0.0f32;
        for i in 0..self.count {
            let (sl, sr_, breath) = self.singers[i].process(f0, v, &self.cached);
            l += sl;
            r += sr_;
            bl += breath * self.singers[i].pan_l;
            br += breath * self.singers[i].pan_r;
        }
        if v.breath > 0.0 {
            if v.breath_shaped {
                let mut sl = 0.0f32;
                let mut sr_ = 0.0f32;
                for f in 0..NUM_FORMANTS {
                    let amp = self.cached[f].2;
                    sl += self.breath_l[f].process(bl) * amp;
                    sr_ += self.breath_r[f].process(br) * amp;
                }
                l += sl * v.breath * BREATH_SHAPED_GAIN;
                r += sr_ * v.breath * BREATH_SHAPED_GAIN;
            } else {
                l += bl * 0.05 * v.breath;
                r += br * 0.05 * v.breath;
            }
        }
        // Singers are decorrelated, so their sum grows as the square root
        // of the count, not the count. The constant anchors four singers at
        // the level they have always had.
        let g = 1.3 / (self.count as f32).sqrt();
        (self.dc[0].process(l * g), self.dc[1].process(r * g))
    }
}

/// Matches the shaped aspiration to the level the flat path produced, so
/// the two differ in colour and not in loudness.
const BREATH_SHAPED_GAIN: f32 = 0.05;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn rms(c: &mut FofChoir, v: &Voicing, f0: f32, n: usize) -> f32 {
        let mut acc = 0.0f64;
        for _ in 0..n {
            let (l, _) = c.process(f0, v);
            acc += (l as f64) * (l as f64);
        }
        (acc / n as f64).sqrt() as f32
    }

    #[test]
    fn produces_audio() {
        let mut c = FofChoir::new(48_000.0);
        c.note_on();
        let v = Voicing::default();
        let mut peak = 0.0f32;
        for _ in 0..48_000 {
            let (l, _r) = c.process(261.6, &v);
            peak = peak.max(l.abs());
        }
        assert!(peak > 0.02, "FOF choir silent (peak {peak})");
    }

    /// Five singers hold their level from the first moments: the onset is
    /// not a coherent burst followed by a dip.
    #[test]
    fn the_singers_do_not_start_phase_locked() {
        let sr = 48_000.0f32;
        let mut c = FofChoir::new(sr);
        c.note_on();
        let v = Voicing { vib_depth: 0.01, singers: 5, ..Default::default() };
        let _ = rms(&mut c, &v, 261.6, (0.02 * sr) as usize);
        let early = rms(&mut c, &v, 261.6, (0.03 * sr) as usize);
        let dip = rms(&mut c, &v, 261.6, (0.15 * sr) as usize);
        let later = rms(&mut c, &v, 261.6, (0.3 * sr) as usize);
        assert!(early < 2.0 * later, "coherent burst: {early} vs {later}");
        assert!(dip > 0.5 * later, "dip: {dip} vs {later}");
    }

    /// Decorrelated sources sum as the square root of their count, so
    /// asking for more singers must not make the choir quieter.
    #[test]
    fn a_bigger_choir_does_not_get_quieter() {
        let sr = 48_000.0f32;
        let level = |n: usize| {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let v = Voicing { singers: n, ..Default::default() };
            let _ = rms(&mut c, &v, 261.6, (0.2 * sr) as usize);
            rms(&mut c, &v, 261.6, sr as usize)
        };
        let four = level(4);
        for n in [8usize, 12, 16] {
            let db = 20.0 * (level(n) / four).log10();
            assert!(db > -2.0, "{n} singers sit {db:.1} dB under four");
            assert!(db < 4.0, "{n} singers sit {db:.1} dB over four");
        }
    }

    /// The walk takes the same path at any sample rate. Singers that drift
    /// together sum louder than singers that drift apart, so a walk that
    /// consumed its randomness per sample would render the same note to a
    /// different level at every rate.
    #[test]
    fn the_walk_does_not_depend_on_the_sample_rate() {
        let level = |sr: f32| {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let v = Voicing { drift_cents: 20.0, breath: 0.0, ..Default::default() };
            let mut acc = 0.0f64;
            let n = (1.5 * sr) as usize;
            for _ in 0..n {
                let (l, _) = c.process(261.6, &v);
                acc += (l as f64) * (l as f64);
            }
            (acc / n as f64).sqrt() as f32
        };
        let a = level(48_000.0);
        let b = level(96_000.0);
        let ratio = b / a.max(1e-9);
        assert!(
            (0.9..1.1).contains(&ratio),
            "the same choir renders at {a} and {b}, a ratio of {ratio}"
        );
    }

    /// A bigger choir is not a narrower one. Singers packed into a fixed
    /// spread land within a cent or two of each other, sing the same
    /// thing, and the sum of them walks back to the middle.
    #[test]
    fn a_bigger_choir_is_not_narrower() {
        let sr = 48_000.0f32;
        let correlation = |n: usize| {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let v = Voicing { singers: n, breath: 0.0, ..Default::default() };
            for _ in 0..(0.2 * sr) as usize {
                let _ = c.process(261.6, &v);
            }
            let (mut ll, mut rr, mut lr) = (0.0f64, 0.0f64, 0.0f64);
            for _ in 0..(2.0 * sr) as usize {
                let (l, r) = c.process(261.6, &v);
                ll += (l * l) as f64;
                rr += (r * r) as f64;
                lr += (l * r) as f64;
            }
            (lr / (ll * rr).sqrt()) as f32
        };
        let four = correlation(4);
        let sixteen = correlation(16);
        assert!(
            sixteen < four + 0.08,
            "sixteen singers sit at {sixteen} against four at {four}"
        );
    }

    /// Every control added to `Voicing` is a bypass at its default, so a
    /// patch written before them renders as it always did.
    #[test]
    fn the_new_controls_are_a_bypass_at_rest() {
        let sr = 48_000.0f32;
        let render = |v: &Voicing| {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let mut out = Vec::with_capacity(24_000);
            for _ in 0..24_000 {
                let (l, r) = c.process(261.6, v);
                out.push(l + r);
            }
            out
        };
        let rest = render(&Voicing::default());
        for (name, v) in [
            ("drift", Voicing { drift_cents: 0.0, ..Default::default() }),
            ("contrast", Voicing { contrast: 0.5, ..Default::default() }),
            ("breath shape", Voicing { breath_shaped: false, ..Default::default() }),
        ] {
            assert_eq!(rest, render(&v), "{name} is not a bypass at rest");
        }
    }

    /// The walk is what spreads a harmonic into a band, so turning it up
    /// has to reach the output.
    #[test]
    fn the_drift_reaches_the_pitch() {
        let sr = 48_000.0f32;
        let render = |cents: f32| {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let mut out = Vec::with_capacity(48_000);
            for _ in 0..48_000 {
                let (l, _) = c.process(261.6, &Voicing { drift_cents: cents, ..Default::default() });
                out.push(l);
            }
            out
        };
        let flat = render(0.0);
        let walked = render(20.0);
        let diff: f32 = flat
            .iter()
            .zip(&walked)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(diff > 1e-3, "twenty cents of drift changed nothing (max diff {diff})");
    }

    /// The choir carries no constant. A grain is a sine cut short and its
    /// mean is only zero when it covers many periods, so a low first
    /// formant leaves an offset behind and every singer leaves the same
    /// one: they add in phase, and the sum steps the output on every note
    /// down and every note up.
    #[test]
    fn the_choir_carries_no_offset() {
        let sr = 48_000.0f32;
        // Ah sits its first formant at 800 Hz and oo at 325; the lower it
        // is, the fewer periods a grain covers and the more it leaves.
        for vowel in [0.0f32, 0.25, 0.5, 0.75, 1.0] {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let v = Voicing { vowel, singers: 8, contrast: 0.9, breath: 0.0, ..Default::default() };
            let mut sum = 0.0f64;
            let mut peak = 0.0f32;
            let n = (2.0 * sr) as usize;
            for _ in 0..(0.2 * sr) as usize {
                let _ = c.process(261.6, &v);
            }
            for _ in 0..n {
                let (l, _) = c.process(261.6, &v);
                sum += l as f64;
                peak = peak.max(l.abs());
            }
            let dc = (sum / n as f64) as f32;
            assert!(
                dc.abs() < 0.02 * peak.max(1e-6),
                "vowel {vowel}: an offset of {dc} under a peak of {peak}"
            );
        }
    }

    /// The air sounds the same at every rate. A noise source is written in
    /// samples and heard in hertz, so nothing that shapes it may carry a
    /// coefficient the rate can move.
    #[test]
    fn the_air_does_not_depend_on_the_sample_rate() {
        let level = |sr: f32, shaped: bool| {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let v = Voicing { breath: 1.0, breath_shaped: shaped, ..Default::default() };
            let mut acc = 0.0f64;
            let n = (1.0 * sr) as usize;
            for _ in 0..(0.2 * sr) as usize {
                let _ = c.process(261.6, &v);
            }
            for _ in 0..n {
                let (l, _) = c.process(261.6, &v);
                acc += (l as f64) * (l as f64);
            }
            (acc / n as f64).sqrt() as f32
        };
        for shaped in [false, true] {
            let a = level(44_100.0, shaped);
            let b = level(96_000.0, shaped);
            let db = 20.0 * (b / a.max(1e-9)).log10();
            assert!(
                db.abs() < 1.0,
                "shaped {shaped}: the air sits {db:.2} dB apart between rates"
            );
        }
    }

    /// The shaped aspiration is a colour, not a level: it must not arrive
    /// louder or quieter than the flat noise it replaces.
    #[test]
    fn shaping_the_breath_keeps_its_level() {
        let sr = 48_000.0f32;
        let level = |shaped: bool| {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let v = Voicing { breath: 1.0, breath_shaped: shaped, ..Default::default() };
            let _ = rms(&mut c, &v, 261.6, (0.2 * sr) as usize);
            rms(&mut c, &v, 261.6, sr as usize)
        };
        let db = 20.0 * (level(true) / level(false)).log10();
        assert!(db.abs() < 3.0, "shaped breath sits {db:.1} dB from the flat one");
    }

    /// cargo test --release --lib fof::tests::profile -- --ignored --nocapture
    #[test]
    #[ignore = "diagnostic"]
    fn profile() {
        let sr = 48_000.0f32;
        for n in [1usize, 4, 8, 16] {
            let mut c = FofChoir::new(sr);
            c.note_on();
            let v = Voicing { singers: n, ..Default::default() };
            for _ in 0..2048 { let _ = c.process(261.6, &v); }
            let blocks = 200_000;
            let t0 = Instant::now();
            for _ in 0..blocks { let _ = c.process(261.6, &v); }
            let ns = t0.elapsed().as_nanos() as f64 / blocks as f64;
            eprintln!("{} singers: {:.1} ns/sample ({:.2}% of one core)",
                      n, ns, ns * sr as f64 / 1e9 * 100.0);
        }
    }
}
