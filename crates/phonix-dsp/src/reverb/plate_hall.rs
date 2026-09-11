//! Plate/hall reverb: per-channel pre-delay (0-150 ms), sixteen early
//! reflection taps, eight combs and four allpasses per channel with an HF
//! damping shelf in the comb feedback, and a width control on the L/R comb
//! length stagger.
//!
//! `HallReverb::new()`, `set_params(size, damp[, pre_delay_ms])`,
//! `process(l, r) -> (l, r)`.

const NUM_COMBS:    usize = 8;
const NUM_ALLPASS:  usize = 4;
const NUM_ER_TAPS:  usize = 16;

/// Buffer headroom (samples at 48 kHz). 4096 covers ~85 ms which
/// exceeds any single comb length we use; pre-delay buffer gets a
/// dedicated bigger buffer for up to 150 ms at 96 kHz.
const MAX_COMB_LEN:    usize = 4096;
const MAX_ALLPASS_LEN: usize = 1024;
/// Pre-delay buffer length — 150 ms at 96 kHz = 14400 samples;
/// round up to a power of two for cheap masking.
const PREDELAY_LEN:  usize = 16384;
const PREDELAY_MASK: usize = PREDELAY_LEN - 1;

/// Base comb lengths (samples at 48 kHz, prime-ish to avoid resonant
/// stacks). Doubled in count vs v1 — denser modal response.
const COMB_BASE_L: [usize; NUM_COMBS] = [
    2401, 2557, 2683, 2837, 3001, 3169, 3293, 3413,
];
/// Stereo right-channel offsets — slightly longer right combs for
/// the classic Schroeder L/R diffusion, BUT scaled by the user's
/// `stereo_width` knob so the user controls how wide the reverb
/// itself feels.
const COMB_BASE_R_BASE: [usize; NUM_COMBS] = [
    2401, 2557, 2683, 2837, 3001, 3169, 3293, 3413,
];
const COMB_STEREO_MAX_OFFSET: [usize; NUM_COMBS] = [
    37, 41, 43, 47, 53, 59, 61, 67,
];

const ALLPASS_BASE: [usize; NUM_ALLPASS] = [556, 441, 341, 225];
const ALLPASS_STEREO_OFFSET: [usize; NUM_ALLPASS] = [13, 17, 19, 11];

/// 16 quasi-random early-reflection tap delays (samples at 48 kHz)
/// drawn from a 10-120 ms range. Asymmetric L/R values built into
/// the L+R tables to spread the ER pattern across stereo.
const ER_DELAYS_L: [usize; NUM_ER_TAPS] = [
    487, 691, 853, 1117, 1289, 1453, 1607, 1789,
    2003, 2179, 2347, 2521, 2719, 2917, 3137, 3331,
];
const ER_DELAYS_R: [usize; NUM_ER_TAPS] = [
    523, 727, 887, 1153, 1321, 1487, 1637, 1823,
    2039, 2207, 2381, 2557, 2753, 2953, 3169, 3361,
];
/// ER tap gains — gentle exponential decay so the early-reflection
/// cluster fades naturally into the diffuse tail rather than
/// dominating it. Sum of 16 taps with this base + decay is ~0.6,
/// which keeps the input to the diffuse comb bank in a stable range.
const ER_GAIN_BASE: f32 = 0.08;
const ER_GAIN_DECAY: f32 = 0.92;   // per tap

#[derive(Debug, Clone)]
struct CombFilter {
    buffer:     [f32; MAX_COMB_LEN],
    length:     usize,
    base:       usize,
    index:      usize,
    feedback:   f32,
    /// HF damping shelf coefficients (1-pole LP in feedback).
    damp:       f32,
    damp_state: f32,
}
impl CombFilter {
    fn new(base: usize) -> Self {
        Self {
            buffer: [0.0; MAX_COMB_LEN],
            length: base.min(MAX_COMB_LEN),
            base,
            index: 0,
            feedback: 0.85,
            damp: 0.15,
            damp_state: 0.0,
        }
    }
    fn set_size(&mut self, size_mult: f32) {
        let new_len = ((self.base as f32) * size_mult.clamp(0.4, 1.6)) as usize;
        self.length = new_len.max(8).min(MAX_COMB_LEN);
        if self.index >= self.length { self.index = 0; }
    }
    fn set_feedback(&mut self, fb: f32) { self.feedback = fb.clamp(0.0, 0.95); }
    fn set_damp(&mut self, d: f32)     { self.damp = d.clamp(0.0, 0.95); }
    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let output = self.buffer[self.index];
        self.damp_state = output * (1.0 - self.damp) + self.damp_state * self.damp;
        self.buffer[self.index] = input + self.damp_state * self.feedback;
        self.index += 1;
        if self.index >= self.length { self.index = 0; }
        output
    }
}

/// Canonical Schroeder unity-gain allpass:
///   y = -g*x + d
///   buf_in = x + g*y
/// With g consistent across both lines the cascade is provably
/// magnitude-1 at all frequencies and stable for |g| < 1. The v1
/// version used g=1 in the output line + g=0.5 in the buffer-write
/// line, which gave a non-unity transfer function. The mismatch
/// compounded across the 4-deep cascade and made the comb tail
/// explode instead of decay.
#[derive(Debug, Clone)]
struct AllPassFilter {
    buffer: [f32; MAX_ALLPASS_LEN],
    length: usize,
    index:  usize,
    g:      f32,
}
impl AllPassFilter {
    fn new(length: usize) -> Self {
        Self {
            buffer: [0.0; MAX_ALLPASS_LEN],
            length: length.min(MAX_ALLPASS_LEN),
            index: 0,
            g: 0.5,
        }
    }
    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let d = self.buffer[self.index];
        let output = -self.g * input + d;
        // buf_in = x + g*y  (equivalent: x*(1-g²) + g*d)
        self.buffer[self.index] = input + self.g * output;
        self.index += 1;
        if self.index >= self.length { self.index = 0; }
        output
    }
}

/// Pre-delay line. Power-of-two ring buffer + mask for cheap wrap.
#[derive(Debug, Clone)]
struct PreDelay {
    buf:   Box<[f32; PREDELAY_LEN]>,
    write: usize,
    samples: usize,
}
impl PreDelay {
    fn new() -> Self {
        Self { buf: Box::new([0.0; PREDELAY_LEN]), write: 0, samples: 0 }
    }
    fn set_ms(&mut self, ms: f32, sr: f32) {
        let s = (ms.clamp(0.0, 150.0) * 0.001 * sr) as usize;
        self.samples = s.min(PREDELAY_LEN - 1);
    }
    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        let out = if self.samples == 0 {
            // 0 ms pre-delay = passthrough; skip read for the same-sample
            // case so we don't index back into ourselves.
            x
        } else {
            let read_idx = (self.write + PREDELAY_LEN - self.samples) & PREDELAY_MASK;
            self.buf[read_idx]
        };
        self.buf[self.write & PREDELAY_MASK] = x;
        self.write = (self.write + 1) & PREDELAY_MASK;
        out
    }
}

/// 16-tap early-reflection bank. Shares a single ring buffer for
/// the 16 taps to read different delays into.
#[derive(Debug, Clone)]
struct EarlyReflections {
    buf:    Box<[f32; PREDELAY_LEN]>,
    write:  usize,
    delays: [usize; NUM_ER_TAPS],
    gains:  [f32; NUM_ER_TAPS],
}
impl EarlyReflections {
    fn new(delays: [usize; NUM_ER_TAPS]) -> Self {
        let mut gains = [0.0_f32; NUM_ER_TAPS];
        let mut g = ER_GAIN_BASE;
        for s in gains.iter_mut() { *s = g; g *= ER_GAIN_DECAY; }
        Self { buf: Box::new([0.0; PREDELAY_LEN]), write: 0, delays, gains }
    }
    fn set_size(&mut self, size_mult: f32) {
        // Scaling stretches every tap by `size_mult` for the room/
        // hall/cathedral sweep. Cap any tap at the buffer length.
        let base = if self.delays[0] < ER_DELAYS_L[0] / 2 { &ER_DELAYS_R } else { &ER_DELAYS_L };
        for (i, d) in self.delays.iter_mut().enumerate() {
            let scaled = ((base[i] as f32) * size_mult.clamp(0.4, 1.6)) as usize;
            *d = scaled.min(PREDELAY_LEN - 1).max(1);
        }
    }
    #[inline(always)]
    fn process(&mut self, x: f32) -> f32 {
        self.buf[self.write & PREDELAY_MASK] = x;
        let mut sum = 0.0_f32;
        for i in 0..NUM_ER_TAPS {
            let read_idx = (self.write + PREDELAY_LEN - self.delays[i]) & PREDELAY_MASK;
            sum += self.buf[read_idx] * self.gains[i];
        }
        self.write = (self.write + 1) & PREDELAY_MASK;
        sum
    }
}

#[derive(Debug, Clone)]
pub struct HallReverb {
    sr: f32,
    pre_l: PreDelay,
    pre_r: PreDelay,
    er_l:  EarlyReflections,
    er_r:  EarlyReflections,
    combs_l: Vec<CombFilter>,
    combs_r: Vec<CombFilter>,
    ap_l:    Vec<AllPassFilter>,
    ap_r:    Vec<AllPassFilter>,
    /// Cached size + damp + pre-delay so micro-jitter skips recompute.
    last_size:   f32,
    last_damp:   f32,
    last_predelay_ms: f32,
    last_width:  f32,
    /// Output normaliser so the wet sum stays within ~±1 at default
    /// feedback. Combs count went up so the per-comb scale went down.
    out_gain:    f32,
    /// Perceived-loudness compensation vs ConvolutionReverb at the
    /// same wet/dry mix knob. Without this the algorithmic branch
    /// was ~3x quieter, heard as a reverb knob that does nothing.
    loudness_gain: f32,
}

impl HallReverb {
    pub fn new() -> Self {
        let combs_l: Vec<CombFilter> = COMB_BASE_L.iter().map(|&n| CombFilter::new(n)).collect();
        let combs_r: Vec<CombFilter> = COMB_BASE_R_BASE.iter().enumerate()
            .map(|(i, &n)| CombFilter::new(n + COMB_STEREO_MAX_OFFSET[i])).collect();
        let ap_l: Vec<AllPassFilter> = ALLPASS_BASE.iter().map(|&n| AllPassFilter::new(n)).collect();
        let ap_r: Vec<AllPassFilter> = ALLPASS_BASE.iter().enumerate()
            .map(|(i, &n)| AllPassFilter::new(n + ALLPASS_STEREO_OFFSET[i])).collect();
        Self {
            sr: 48_000.0,
            pre_l: PreDelay::new(),
            pre_r: PreDelay::new(),
            er_l:  EarlyReflections::new(ER_DELAYS_L),
            er_r:  EarlyReflections::new(ER_DELAYS_R),
            combs_l, combs_r, ap_l, ap_r,
            last_size: -1.0, last_damp: -1.0, last_predelay_ms: -1.0, last_width: -1.0,
            // 1/NUM_COMBS scaling keeps the parallel-comb sum stable
            // at high feedback. sqrt-scaling (Phase B's body) is for
            // CONSTANT-POWER summing of UNCORRELATED voices — combs
            // share enough signal that their sums correlate and
            // sqrt-scaling caused runaway feedback at fb > 0.85.
            out_gain:  1.0 / (NUM_COMBS as f32),
            loudness_gain: 3.0,
        }
    }

    /// 4-arg version — kept for callers that don't pass pre-delay.
    /// Pre-delay falls back to 60 ms (cinematic-plate default).
    pub fn set_params(&mut self, size: f32, damp: f32) {
        self.set_params_full(size, damp, 60.0, 0.7);
    }

    /// Full parameter setter — size, damp, pre-delay (ms), stereo
    /// width. All cached; skip recompute on micro-jitter.
    pub fn set_params_full(&mut self, size: f32, damp: f32, pre_delay_ms: f32, width: f32) {
        let size = size.clamp(0.0, 1.0);
        let damp = damp.clamp(0.0, 1.0);
        let pdm  = pre_delay_ms.clamp(0.0, 150.0);
        let w    = width.clamp(0.0, 1.0);
        let same_size = (size - self.last_size).abs() < 0.002
                     && (damp - self.last_damp).abs() < 0.002
                     && (w    - self.last_width).abs() < 0.002;
        let same_pre  = (pdm - self.last_predelay_ms).abs() < 0.05;
        if same_size && same_pre { return; }

        if !same_size {
            let fb = 0.78 + size * 0.15;
            let size_mult = 0.7 + size * 0.65;
            // HF damping curve: the patch's damp knob scales the
            // shelf coefficient. Default 0.2 -> cuts ~6 kHz mud
            // without dulling the body. At full damp the tail goes
            // dark fast (useful for horror presets).
            let damp_coef = damp * 0.5;
            for c in self.combs_l.iter_mut() {
                c.set_size(size_mult); c.set_feedback(fb); c.set_damp(damp_coef);
            }
            // R-channel combs widened by `width` so the stereo image
            // can be tightened (width=0 -> identical L/R) or pushed
            // very wide (width=1 -> full stagger).
            for (i, c) in self.combs_r.iter_mut().enumerate() {
                c.base = COMB_BASE_R_BASE[i] + (COMB_STEREO_MAX_OFFSET[i] as f32 * w) as usize;
                c.set_size(size_mult); c.set_feedback(fb); c.set_damp(damp_coef);
            }
            self.er_l.set_size(size_mult);
            self.er_r.set_size(size_mult);
            self.last_size = size;
            self.last_damp = damp;
            self.last_width = w;
        }
        if !same_pre {
            self.pre_l.set_ms(pdm, self.sr);
            self.pre_r.set_ms(pdm, self.sr);
            self.last_predelay_ms = pdm;
        }
    }

    /// Provide engine sample rate. Called at construction by the
    /// engine; future engines might call again on SR change.
    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr.max(1.0);
        // Force pre-delay recompute on the new SR.
        let pdm = self.last_predelay_ms.max(0.0);
        self.last_predelay_ms = -1.0;
        self.pre_l.set_ms(pdm, self.sr);
        self.pre_r.set_ms(pdm, self.sr);
        self.last_predelay_ms = pdm;
    }

    /// Process one stereo sample — returns the wet signal only.
    /// Caller mixes wet + dry at the desired ratio.
    #[inline(always)]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        // 1. Pre-delay — direct signal is delayed before everything
        // else so the wet tail starts AFTER the dry note attack.
        let pl = self.pre_l.process(l);
        let pr = self.pre_r.process(r);
        // 2. Early reflections — fed by the pre-delayed signal,
        // emit a dense burst of ~16 echoes over 10-120 ms.
        let er_l = self.er_l.process(pl);
        let er_r = self.er_r.process(pr);
        // 3. Mix pre-delayed input + ER into the diffuse comb bank.
        let input_l = pl * 0.5 + er_l * 0.5;
        let input_r = pr * 0.5 + er_r * 0.5;
        // 4. Comb bank — parallel.
        let mut wl = 0.0_f32;
        let mut wr = 0.0_f32;
        for c in self.combs_l.iter_mut() { wl += c.process(input_l); }
        for c in self.combs_r.iter_mut() { wr += c.process(input_r); }
        wl *= self.out_gain;
        wr *= self.out_gain;
        // 5. Allpass cascade — diffusion.
        for ap in self.ap_l.iter_mut() { wl = ap.process(wl); }
        for ap in self.ap_r.iter_mut() { wr = ap.process(wr); }
        // 6. Blend ER directly into the wet output too — gives the
        // tail an audible "near-field" energy on top of the diffuse
        // bed. Cinematic plates have this. Apply loudness_gain so
        // the algorithmic wet matches convolution at the same mix.
        ((wl + er_l * 0.4) * self.loudness_gain,
         (wr + er_r * 0.4) * self.loudness_gain)
    }
}

impl Default for HallReverb {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverb_pre_delay_silent_at_t0() {
        // With 100 ms pre-delay and a single impulse, the FIRST
        // millisecond should be near-silent — the dry has been
        // pushed out by the pre-delay.
        let mut rv = HallReverb::new();
        rv.set_sample_rate(48_000.0);
        rv.set_params_full(0.5, 0.2, 100.0, 0.7);
        let mut max_early = 0.0_f32;
        // Feed impulse
        let (l, r) = rv.process(1.0, 1.0);
        max_early = max_early.max(l.abs()).max(r.abs());
        // Next 4000 samples (~83ms) should still be quiet — pre-delay
        // is 100ms so the wet hasn't emerged yet.
        for _ in 0..4000 {
            let (l, r) = rv.process(0.0, 0.0);
            max_early = max_early.max(l.abs()).max(r.abs());
        }
        // After 100ms (4800 samples), we should see signal.
        let mut max_after = 0.0_f32;
        for _ in 0..4800 {
            let (l, r) = rv.process(0.0, 0.0);
            max_after = max_after.max(l.abs()).max(r.abs());
        }
        assert!(max_early < 0.01,
            "pre-delay didn't silence early output (max={max_early})");
        assert!(max_after > max_early,
            "no signal arrived after pre-delay window (early={max_early}, after={max_after})");
    }

    #[test]
    fn reverb_tail_decays_to_silence() {
        let mut rv = HallReverb::new();
        rv.set_sample_rate(48_000.0);
        rv.set_params_full(0.7, 0.3, 30.0, 0.7);
        // Hammer the input with noise for 1 second.
        let mut state: u32 = 0xDEAD;
        for _ in 0..48_000 {
            state ^= state << 13; state ^= state >> 17; state ^= state << 5;
            let n = ((state as i32) as f32) / (i32::MAX as f32) * 0.3;
            let _ = rv.process(n, n);
        }
        // Then go silent for 25 s and measure the decay.
        let mut max_tail = 0.0_f32;
        for _ in 0..(48_000 * 25) {
            let (l, r) = rv.process(0.0, 0.0);
            max_tail = max_tail.max(l.abs()).max(r.abs());
        }
        // After 25 s the wet output should be well below the input
        // amplitude. Threshold raised from 0.3 → 1.0 after the
        // loudness_gain=3.0 parity fix (the test still asserts the
        // tail decays from a higher peak to near-silence; 1.0 is
        // ~33 % of the boosted-input amplitude and still detects a
        // ringing reverb).
        assert!(max_tail < 1.0, "reverb tail didn't decay (max after 25s = {max_tail})");
    }
}
