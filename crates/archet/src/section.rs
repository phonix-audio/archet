//! Archet SECTION model - turn the small real-voice pool into a large
//! string section at fixed (size-independent) cost.
//!
//! Why not N physical voices: rendering 100 bowed waveguides is impossible
//! and perceptually pointless (Ternstroem: independent-source richness
//! saturates by ~8-12 voices). The engine renders a bounded pool of real
//! decorrelated voices (true physical timbre + ~14-cent static F0 scatter);
//! this module thickens it toward the large-section texture, O(1) in size.
//!
//! ALGORITHM = the classic string-ensemble CHORUS (Roland/ARP BBD ensemble,
//! Solina; Dattorro "Effect Design"; Zoelzer DAFX modulation chapter), NOT
//! an all-pass diffuser and NOT a fixed/velvet delay. Each extra "player" is
//! a copy read from a delay line whose length is MODULATED slowly, so the
//! copy is DOPPLER-DETUNED by a few cents -- exactly like a real player at a
//! slightly different, wandering pitch. Detuned (not merely delayed) copies
//! broaden each partial into a band (no static comb notches: the residual
//! comb continuously SWEEPS = the natural ensemble shimmer, not metallic
//! coloration). Independent per-tap modulation phases give the vibrato/pitch
//! ASYNCHRONY that is the primary "many players" cue. Cost = N taps, flat.

const N_TAPS: usize = 8;     // extra detuned copies = perceived players
const BASE_MIN_MS: f32 = 12.0;
const BASE_MAX_MS: f32 = 30.0;
const DEPTH_MS: f32 = 3.4;   // modulation depth -> ~18-20 cents peak detune,
                             // matching the measured 20-30c section dispersion

struct Tap {
    base: f32,           // base delay, samples
    ph: [f32; 3],        // 3 incommensurate LFO phases
    inc: [f32; 3],       // per-sample phase increments
    pan_l: f32,
    pan_r: f32,
}

pub struct SectionDiffuser {
    buf: Vec<f32>,   // mono (mid) history
    w: usize,
    taps: [Tap; N_TAPS],
    depth: f32,      // samples
    norm: f32,
    lp_l: f32,       // low-shelf state (body/weight that grows with size)
    lp_r: f32,
    lp_coeff: f32,
}

impl SectionDiffuser {
    pub fn new(sr: f32) -> Self {
        // deterministic per-tap LFO rates/phases (no Math.random): three
        // incommensurate low rates near 0.3-0.8 Hz, jittered per tap, so no
        // two copies ever share a pitch trajectory (asynchrony).
        let mut seed = 0x2545_F491u32;
        let mut rnd = || {
            seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let base_rates = [0.31f32, 0.47, 0.69];
        let mk = |k: usize, rnd: &mut dyn FnMut() -> f32| -> Tap {
            let frac = if N_TAPS > 1 { k as f32 / (N_TAPS - 1) as f32 } else { 0.5 };
            let base_ms = BASE_MIN_MS + frac * (BASE_MAX_MS - BASE_MIN_MS);
            let mut inc = [0.0f32; 3];
            let mut ph = [0.0f32; 3];
            for j in 0..3 {
                let rate = base_rates[j] * (0.8 + 0.4 * rnd()); // jitter +/-20%
                inc[j] = std::f32::consts::TAU * rate / sr;
                ph[j] = rnd() * std::f32::consts::TAU;
            }
            // seat the copy across the desk (constant power)
            let pan = frac * 2.0 - 1.0;
            let ang = (pan * 0.9 + 1.0) * 0.5 * std::f32::consts::FRAC_PI_2;
            Tap { base: base_ms * 0.001 * sr, ph, inc, pan_l: ang.cos(), pan_r: ang.sin() }
        };
        let taps: [Tap; N_TAPS] = std::array::from_fn(|k| mk(k, &mut rnd));
        let cap = (((BASE_MAX_MS + DEPTH_MS) * 0.001 * sr) as usize + 8).next_power_of_two();
        Self {
            buf: vec![0.0; cap],
            w: 0,
            taps,
            depth: DEPTH_MS * 0.001 * sr,
            norm: (N_TAPS as f32).sqrt().recip(),
            lp_l: 0.0,
            lp_r: 0.0,
            lp_coeff: 1.0 - (-std::f32::consts::TAU * 260.0 / sr).exp(), // ~260 Hz
        }
    }

    pub fn reset(&mut self) {
        for s in self.buf.iter_mut() { *s = 0.0; }
        self.w = 0;
        self.lp_l = 0.0;
        self.lp_r = 0.0;
    }

    /// Thicken a stereo input into a section. `fill` 0..1 = size beyond the
    /// real-voice pool; 0 = bypass. The wet is N Doppler-detuned, panned
    /// copies (density + width); equal-power blend keeps the level bounded.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32, fill: f32) -> (f32, f32) {
        if fill < 1e-4 {
            return (l, r);
        }
        let mid = 0.5 * (l + r);
        self.buf[self.w] = mid;
        let len = self.buf.len();
        let w = self.w;
        let depth = self.depth;

        let (mut wl, mut wr) = (0.0f32, 0.0f32);
        for t in self.taps.iter_mut() {
            let m = t.ph[0].sin() + 0.6 * t.ph[1].sin() + 0.4 * t.ph[2].sin();
            let d = (t.base + depth * m).clamp(1.0, (len - 2) as f32);
            let di = d as usize;
            let frac = d - di as f32;
            let i0 = (w + len - di) % len;
            let i1 = (w + len - di - 1) % len;
            let c = self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac;
            wl += c * t.pan_l;
            wr += c * t.pan_r;
            for j in 0..3 {
                t.ph[j] += t.inc[j];
                if t.ph[j] >= std::f32::consts::TAU { t.ph[j] -= std::f32::consts::TAU; }
            }
        }
        wl *= self.norm;
        wr *= self.norm;

        self.w = (self.w + 1) % len;

        // SIZE -> WEIGHT. The detuned-copy layer is ADDED on top of the full
        // dry pool (not equal-power-swapped) so a bigger section is genuinely
        // denser AND heavier (more bows = more energy: the "poids des notes"),
        // and a low-shelf body grows with size for spectral DEPTH. Both scale
        // with fill, so 100 is audibly weightier/deeper than 14, at flat cost.
        let wet_gain = 0.20 + 0.95 * fill;
        let mut ol = l + wl * wet_gain;
        let mut or_ = r + wr * wet_gain;
        self.lp_l += (ol - self.lp_l) * self.lp_coeff;
        self.lp_r += (or_ - self.lp_r) * self.lp_coeff;
        ol += self.lp_l * 0.55 * fill; // low-mid weight that fills in with size
        or_ += self.lp_r * 0.55 * fill;
        (ol, or_)
    }
}
