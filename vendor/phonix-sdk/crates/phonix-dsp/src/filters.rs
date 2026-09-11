//! Shared filter primitives, hoisted out of individual engines so any plugin
//! can compose them.
//!
//! # These are NOT interchangeable
//!
//! Before this module existed the codebase held ~20 filter structs, several
//! of them called `SvfFilter` while implementing genuinely different maths.
//! That ambiguity is what produced the copies in the first place, so names
//! here say what the filter *is*, never what it is *for*:
//!
//! * [`Biquad`] — direct-form 1, RBJ cookbook coefficients.
//! * [`BiquadT`] — transposed direct-form 2. Same transfer function as
//!   [`Biquad`], different rounding: **the two are not bit-identical**. Pick
//!   one per signal path and stay with it.
//!
//! Two SVF topologies are in use and are likewise algebraically equal but not
//! bit-identical, because the operation order differs. Swapping one for the
//! other changes the rendered output, so they keep separate names and neither
//! is "the" SVF.
//!
//! When adding a filter here, prefer widening an existing type over adding a
//! near-copy — but never at the cost of changing an existing engine's output.

/// Biquad, direct-form 1, with RBJ-cookbook coefficient setters.
///
/// `process` returns 0 and self-resets if the state goes non-finite, so a
/// blown-up coefficient set cannot poison a whole render with NaN.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    x1: f32, x2: f32, y1: f32, y2: f32,
}

impl Default for Biquad {
    fn default() -> Self { Self::new() }
}

impl Biquad {
    pub fn new() -> Self {
        Self { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0, x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0 }
    }

    pub fn reset(&mut self) { self.x1 = 0.0; self.x2 = 0.0; self.y1 = 0.0; self.y2 = 0.0; }

    /// Set the coefficients directly (already normalized by a0).
    pub fn set_coeffs(&mut self, b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) {
        self.b0 = b0; self.b1 = b1; self.b2 = b2; self.a1 = a1; self.a2 = a2;
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1; self.x1 = x;
        self.y2 = self.y1; self.y1 = y;
        if y.is_finite() { y } else { self.reset(); 0.0 }
    }

    /// Configure as peaking EQ (RBJ cookbook).
    pub fn set_peak(&mut self, fs: f32, freq: f32, q: f32, gain_db: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / fs;
        let cosw = w0.cos();
        let sinw = w0.sin();
        let alpha = sinw / (2.0 * q.max(0.1));
        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cosw;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = -2.0 * cosw;
        let a2 = 1.0 - alpha / a;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Configure as low shelf (RBJ).
    pub fn set_low_shelf(&mut self, fs: f32, freq: f32, q: f32, gain_db: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / fs;
        let cosw = w0.cos();
        let sinw = w0.sin();
        let alpha = sinw / (2.0 * q.max(0.1));
        let beta = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) - (a - 1.0) * cosw + beta);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cosw);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cosw - beta);
        let a0 = (a + 1.0) + (a - 1.0) * cosw + beta;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cosw);
        let a2 = (a + 1.0) + (a - 1.0) * cosw - beta;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Configure as high shelf (RBJ).
    pub fn set_high_shelf(&mut self, fs: f32, freq: f32, q: f32, gain_db: f32) {
        let a = 10.0f32.powf(gain_db / 40.0);
        let w0 = 2.0 * std::f32::consts::PI * freq / fs;
        let cosw = w0.cos();
        let sinw = w0.sin();
        let alpha = sinw / (2.0 * q.max(0.1));
        let beta = 2.0 * a.sqrt() * alpha;
        let b0 = a * ((a + 1.0) + (a - 1.0) * cosw + beta);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cosw);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cosw - beta);
        let a0 = (a + 1.0) - (a - 1.0) * cosw + beta;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cosw);
        let a2 = (a + 1.0) - (a - 1.0) * cosw - beta;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Configure as bandpass (constant 0 dB peak, RBJ).
    pub fn set_bandpass(&mut self, fs: f32, freq: f32, q: f32) {
        let w0 = 2.0 * std::f32::consts::PI * freq / fs;
        let cosw = w0.cos();
        let sinw = w0.sin();
        let alpha = sinw / (2.0 * q.max(0.1));
        let b0 = alpha;
        let b1 = 0.0;
        let b2 = -alpha;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cosw;
        let a2 = 1.0 - alpha;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Configure as a 2nd-order lowpass (RBJ cookbook).
    pub fn set_lowpass(&mut self, fs: f32, freq: f32, q: f32) {
        let w0 = 2.0 * std::f32::consts::PI * freq / fs;
        let cosw = w0.cos();
        let sinw = w0.sin();
        let alpha = sinw / (2.0 * q.max(0.1));
        let b1 = 1.0 - cosw;
        let b0 = b1 / 2.0;
        let b2 = b0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cosw;
        let a2 = 1.0 - alpha;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Configure as a 2nd-order highpass (RBJ cookbook).
    pub fn set_highpass(&mut self, fs: f32, freq: f32, q: f32) {
        let w0 = 2.0 * std::f32::consts::PI * freq / fs;
        let cosw = w0.cos();
        let sinw = w0.sin();
        let alpha = sinw / (2.0 * q.max(0.1));
        let b0 = (1.0 + cosw) / 2.0;
        let b1 = -(1.0 + cosw);
        let b2 = (1.0 + cosw) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cosw;
        let a2 = 1.0 - alpha;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Evaluate |H(f)| at `freq` — for visualizing the curve in a GUI.
    /// Returns the magnitude in linear scale (1.0 = 0 dB).
    pub fn magnitude_at(&self, fs: f32, freq: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * freq / fs;
        let (sw, cw) = w.sin_cos();
        // H(z) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)
        // Evaluate at z = e^(jw): z^-1 = cos(-w) + j sin(-w) = cos(w) - j sin(w)
        let cw2 = (2.0 * w).cos(); let sw2 = (2.0 * w).sin();
        let num_re = self.b0 + self.b1 * cw + self.b2 * cw2;
        let num_im = -(self.b1 * sw + self.b2 * sw2);
        let den_re = 1.0 + self.a1 * cw + self.a2 * cw2;
        let den_im = -(self.a1 * sw + self.a2 * sw2);
        let num_mag = (num_re * num_re + num_im * num_im).sqrt();
        let den_mag = (den_re * den_re + den_im * den_im).sqrt().max(1e-12);
        num_mag / den_mag
    }
}

/// Biquad, transposed direct-form 2.
///
/// Same transfer function as [`Biquad`], different rounding — the two are not
/// bit-identical, so a signal path must pick one and keep it.
///
/// This carries the state machine only. Coefficient derivation stays with the
/// caller on purpose: the engines using DF2T do not agree on how to get there
/// (`2.0 * PI * (f0 / fs)` rounds differently from `2.0 * PI * f0 / fs` for 35%
/// of inputs, and their Q floors range from 0.05 to 0.3). Sharing the setters
/// would silently re-voice whichever engine lost the coin toss, so build your
/// coefficients locally and hand them over with [`BiquadT::set_coeffs`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BiquadT {
    b0: f32, b1: f32, b2: f32,
    a1: f32, a2: f32,
    z1: f32, z2: f32,
}

impl BiquadT {
    pub fn new() -> Self { Self::default() }

    /// Un-normalized RBJ-style coefficients; `a0` divides the rest.
    pub fn from_coeffs(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, z1: 0.0, z2: 0.0 }
    }

    /// Already-normalized coefficients, leaving the state untouched.
    pub fn set_coeffs(&mut self, b0: f32, b1: f32, b2: f32, a1: f32, a2: f32) {
        self.b0 = b0; self.b1 = b1; self.b2 = b2; self.a1 = a1; self.a2 = a2;
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    pub fn reset(&mut self) { self.z1 = 0.0; self.z2 = 0.0; }
}

/// State-variable filter, 2-pole / 12 dB per octave, Cytomic ("a1/a2/a3") form.
///
/// Returns all four taps at once — a caller that wants one picks it, and the
/// unused ones fold away once inlined. Resonance is normalized 0..1 and maps to
/// `k = 2 - 2r`, so 0.99 is the practical self-oscillation ceiling.
///
/// **Not** interchangeable with a Zavalishin/TPT "hp-first" SVF: same transfer
/// function, different operation order, different bits. If you need the other
/// one, add it under its own name rather than reworking this one.
#[derive(Debug, Clone, Copy)]
pub struct SvfCytomic {
    ic1eq: f32,
    ic2eq: f32,
    sample_rate: f32,
}

impl SvfCytomic {
    pub fn new(sample_rate: f32) -> Self {
        Self { ic1eq: 0.0, ic2eq: 0.0, sample_rate }
    }

    pub fn reset(&mut self) {
        self.ic1eq = 0.0;
        self.ic2eq = 0.0;
    }

    /// Process and return (low, high, band, notch).
    #[inline(always)]
    pub fn process(&mut self, input: f32, cutoff: f32, resonance: f32) -> (f32, f32, f32, f32) {
        let freq = cutoff.clamp(20.0, self.sample_rate * 0.45);
        let g = (std::f32::consts::PI * freq / self.sample_rate).tan();
        let k = 2.0 - 2.0 * resonance.clamp(0.0, 0.99);

        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;

        let v3 = input - self.ic2eq;
        let v1 = a1 * self.ic1eq + a2 * v3;
        let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;

        self.ic1eq = 2.0 * v1 - self.ic1eq;
        self.ic2eq = 2.0 * v2 - self.ic2eq;

        let low = v2;
        let band = v1;
        let high = input - k * v1 - v2;
        let notch = input - k * v1;

        (low, high, band, notch)
    }
}

/// State-variable filter, 2-pole, Zavalishin TPT ("hp-first") form, with a
/// coefficient cache.
///
/// Resonance is normalized 0..1 and maps to `q = 0.5 + r² * 39.5`, so the top
/// of the knob self-oscillates. Coefficients are recomputed only when cutoff
/// moves more than 0.5 Hz or resonance more than 0.001 — `tan()` per sample is
/// what this cache exists to avoid.
///
/// **Not** interchangeable with [`SvfCytomic`]: same transfer function, but the
/// operation order differs and so do the bits. Neither is "the" SVF.
#[derive(Debug, Clone, Copy, Default)]
pub struct SvfTpt {
    sample_rate: f32,
    ic1eq: f32,
    ic2eq: f32,
    cached_cutoff: f32,
    cached_res: f32,
    g: f32,
    k: f32,
}

impl SvfTpt {
    pub fn new(sample_rate: f32) -> Self {
        let mut f = Self {
            sample_rate, ic1eq: 0.0, ic2eq: 0.0,
            cached_cutoff: -1.0, cached_res: -1.0,
            g: 0.0, k: 1.0,
        };
        f.refresh(800.0, 0.3);
        f
    }

    #[inline]
    fn refresh(&mut self, cutoff: f32, res: f32) {
        let c = cutoff.clamp(20.0, 0.49 * self.sample_rate);
        let r = res.clamp(0.0, 1.0);
        self.g = (std::f32::consts::PI * c / self.sample_rate).tan();
        // q from 0..1 -> 0.5..40. High end self-oscillates.
        let q = 0.5 + r * r * 39.5;
        self.k = 1.0 / q;
        self.cached_cutoff = c;
        self.cached_res = r;
    }

    /// Recompute coefficients only if the request moved enough to matter.
    #[inline]
    pub fn refresh_if_needed(&mut self, cutoff: f32, res: f32) {
        if (cutoff - self.cached_cutoff).abs() > 0.5
            || (res - self.cached_res).abs() > 0.001
        {
            self.refresh(cutoff, res);
        }
    }

    /// Tick with the coefficients already set. Returns (lp, hp, bp).
    /// Notch = input - bp.
    #[inline(always)]
    pub fn tick(&mut self, x: f32) -> (f32, f32, f32) {
        let g = self.g;
        let k = self.k;
        let one_plus_g_k = 1.0 + g * (g + k);
        let hp = (x - self.ic2eq - (g + k) * self.ic1eq) / one_plus_g_k;
        let bp = g * hp + self.ic1eq;
        let lp = g * bp + self.ic2eq;
        self.ic1eq = g * hp + bp;
        self.ic2eq = g * bp + lp;
        (lp, hp, bp)
    }

    /// Refresh-then-tick, for callers that carry cutoff/resonance per sample.
    #[inline(always)]
    pub fn tick_at(&mut self, x: f32, cutoff: f32, res: f32) -> (f32, f32, f32) {
        self.refresh_if_needed(cutoff, res);
        self.tick(x)
    }

    pub fn reset(&mut self) { self.ic1eq = 0.0; self.ic2eq = 0.0; }
}

// ─── Ladders ─────────────────────────────────────────────────────────────────
//
// Five 4-pole ladders live here and NONE of them is a copy of another. They
// differ in topology, in coefficient derivation, and above all in where the
// nonlinearity sits — which is exactly what gives each host engine its voice.
// Merging them would not be a refactor, it would be a re-voicing. Pick by
// character, not by name length.

/// One-pole cascade coefficient: `g / (1 + g)` with `g = tan(PI * fc)`.
/// Shared by [`LadderCascadeTanh`] and [`LadderCascadeDiode`], which derive it
/// identically.
#[inline(always)]
pub fn g1_from_cutoff(cutoff: f32, sample_rate: f32) -> f32 {
    let fc = (cutoff / sample_rate).clamp(0.001, 0.49);
    let g = (std::f32::consts::PI * fc).tan();
    g / (1.0 + g)
}

/// 4-pole (24 dB/oct) ladder: linear one-pole cascade, `tanh` on the feedback
/// path only. Dark and forgiving; the saturation never touches the stages.
///
/// Self-oscillates from resonance ~0.55 upwards and stays bounded to 1.0 (peak
/// ~2.2) because the feedback is tanh-limited.
///
/// The feedback is subtracted TWICE, so effective feedback is `resonance * 8`.
/// That is deliberate and load-bearing: 4 is the textbook threshold for a
/// *linear* loop, but the tanh eats enough gain that 4 alone never oscillates
/// at all. Do not fold the two subtractions into `input - 2.0 * fb * feedback`
/// either — one rounding is not two, and 21% of samples come out with different
/// bits.
#[derive(Debug, Clone, Default)]
pub struct LadderCascadeTanh {
    stage: [f32; 4],
    sample_rate: f32,
}

impl LadderCascadeTanh {
    pub fn new(sample_rate: f32) -> Self {
        Self { stage: [0.0; 4], sample_rate }
    }

    pub fn reset(&mut self) { self.stage = [0.0; 4]; }

    #[inline(always)]
    pub fn process(&mut self, input: f32, cutoff: f32, resonance: f32) -> f32 {
        let g1 = g1_from_cutoff(cutoff, self.sample_rate);

        let fb = resonance * 4.0;
        let feedback = self.stage[3].tanh();

        // Twice on purpose — see the type doc.
        let input = input - fb * feedback;
        let s = input - fb * feedback;

        for i in 0..4 {
            let inp = if i == 0 { s } else { self.stage[i - 1] };
            self.stage[i] += (inp - self.stage[i]) * g1;
        }

        self.stage[3]
    }
}

/// 4-pole ladder with a diode clipper applied to the feedback *and* to every
/// stage input. Softer than `tanh` above unity and it colours each pole, which
/// is where the nasal, hollow character comes from.
///
/// Note there is no `reset`: the host that uses it never needed one, and adding
/// it now would not change any render — but do not assume the state is clean.
#[derive(Debug, Clone, Default)]
pub struct LadderCascadeDiode {
    stage: [f32; 4],
    feedback: f32,
    sample_rate: f32,
}

impl LadderCascadeDiode {
    pub fn new(sample_rate: f32) -> Self {
        Self { stage: [0.0; 4], feedback: 0.0, sample_rate }
    }

    pub fn reset(&mut self) { self.stage = [0.0; 4]; self.feedback = 0.0; }

    /// Softer than tanh: identity below 1, gentle compression above.
    #[inline(always)]
    fn diode_clip(x: f32) -> f32 {
        if x.abs() < 1.0 { x } else { x / (1.0 + x.abs() * 0.3) }
    }

    #[inline(always)]
    pub fn process(&mut self, input: f32, cutoff: f32, resonance: f32) -> f32 {
        let g1 = g1_from_cutoff(cutoff, self.sample_rate);

        let fb = resonance * 4.0;
        let inp = input - fb * Self::diode_clip(self.feedback);

        let mut x = inp;
        for i in 0..4 {
            x = Self::diode_clip(x);
            self.stage[i] += (x - self.stage[i]) * g1;
            x = self.stage[i];
        }

        self.feedback = self.stage[3];
        self.stage[3]
    }
}

/// Huovilainen Moog ladder: a fast rational `tanh` approximation on every
/// stage, plus the classic `1 - 0.15 f²` resonance compensation and the 1.16
/// frequency tuning fudge. The most "Moog" of the five.
#[derive(Debug, Clone)]
pub struct LadderHuovilainen {
    stage: [f32; 4],
    stage_tanh: [f32; 4],
    delay: [f32; 4],
    sample_rate: f32,
}

impl LadderHuovilainen {
    pub fn new(sample_rate: f32) -> Self {
        Self { stage: [0.0; 4], stage_tanh: [0.0; 4], delay: [0.0; 4], sample_rate }
    }

    pub fn reset(&mut self) {
        self.stage = [0.0; 4];
        self.stage_tanh = [0.0; 4];
        self.delay = [0.0; 4];
    }

    /// Fast tanh approximation.
    #[inline(always)]
    fn tanh_approx(x: f32) -> f32 {
        let x = x.clamp(-3.0, 3.0);
        x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
    }

    pub fn process(&mut self, input: f32, cutoff: f32, resonance: f32) -> f32 {
        let freq = cutoff.clamp(20.0, self.sample_rate * 0.45);
        let fc = freq / self.sample_rate;
        let f = fc * 1.16; // Frequency tuning coefficient
        let fb = resonance * 4.0 * (1.0 - 0.15 * f * f); // Resonance compensation

        let input = input - fb * self.delay[3];
        let input = Self::tanh_approx(input);

        for i in 0..4 {
            let prev = if i == 0 { input } else { self.stage_tanh[i - 1] };
            self.stage[i] = self.stage[i] + f * (prev - self.stage[i]);
            self.stage_tanh[i] = Self::tanh_approx(self.stage[i]);
        }

        for i in 0..4 {
            self.delay[i] = self.stage_tanh[i];
        }

        self.stage[3]
    }
}

/// 4-pole TPT ladder with a bilinear pre-warp, `tanh` on the feedback, an
/// optional `tanh` pre-filter drive, and a coefficient cache.
///
/// The only ladder here that takes `drive` in its `process` — pre-filter
/// saturation is part of its voice, not an effect bolted in front. Output is
/// scaled by `1 + k * 0.4` to make up the gain a ladder loses as resonance
/// climbs, so it stays level across the knob where the others do not.
#[derive(Debug, Clone, Default)]
pub struct LadderTpt {
    sample_rate: f32,
    /// TPT state, one per stage.
    s: [f32; 4],
    cached_cutoff: f32,
    cached_res: f32,
    /// pre-warped gain
    g: f32,
    /// resonance feedback gain
    k: f32,
}

impl LadderTpt {
    pub fn new(sample_rate: f32) -> Self {
        let mut f = Self {
            sample_rate, s: [0.0; 4],
            cached_cutoff: -1.0, cached_res: -1.0,
            g: 0.0, k: 0.0,
        };
        f.update_coefficients(800.0, 0.3);
        f
    }

    #[inline]
    fn update_coefficients(&mut self, cutoff: f32, res: f32) {
        let c = cutoff.clamp(20.0, 0.49 * self.sample_rate);
        let r = res.clamp(0.0, 1.0);
        // Bilinear pre-warp.
        let wd = std::f32::consts::TAU * c;
        let t = 1.0 / self.sample_rate;
        let wa = (2.0 / t) * (wd * t * 0.5).tan();
        self.g = wa * t * 0.5;
        // 0..1 resonance -> 0..~4 feedback gain (just past self-oscillation
        // at the top).
        self.k = r * 4.0;
        self.cached_cutoff = c;
        self.cached_res = r;
    }

    /// `cutoff_hz` and `resonance` may modulate per sample; the cache avoids
    /// paying for `tan()` and the divisions when they barely move. `drive = 0`
    /// is identity.
    #[inline(always)]
    pub fn process(&mut self, input: f32, cutoff_hz: f32, resonance: f32, drive: f32) -> f32 {
        let cutoff_diff = (cutoff_hz - self.cached_cutoff).abs();
        let res_diff = (resonance - self.cached_res).abs();
        if cutoff_diff > 0.5 || res_diff > 0.001 {
            self.update_coefficients(cutoff_hz, resonance);
        }

        let driven = if drive > 0.0001 {
            let amt = 1.0 + drive * 4.0;
            (input * amt).tanh()
        } else {
            input
        };

        let fb = (self.s[3] * self.k).tanh();
        let x = driven - fb;

        // Four cascaded TPT integrators: y_n = (g * x + s_n) / (1 + g)
        let g = self.g;
        let one_plus_g = 1.0 + g;

        let y0 = (g * x  + self.s[0]) / one_plus_g;
        self.s[0] = 2.0 * y0 - self.s[0];

        let y1 = (g * y0 + self.s[1]) / one_plus_g;
        self.s[1] = 2.0 * y1 - self.s[1];

        let y2 = (g * y1 + self.s[2]) / one_plus_g;
        self.s[2] = 2.0 * y2 - self.s[2];

        let y3 = (g * y2 + self.s[3]) / one_plus_g;
        self.s[3] = 2.0 * y3 - self.s[3];

        y3 * (1.0 + self.k * 0.4)
    }

    pub fn reset(&mut self) { self.s = [0.0; 4]; }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        // Force re-cache on next process.
        self.cached_cutoff = -1.0;
        self.cached_res = -1.0;
        self.reset();
    }
}

/// Diode ladder, Open303's TB_303 branch: cross-coupled stages, a 150 Hz
/// highpass inside the feedback path (the squelch), and polynomial coefficient
/// fits measured off the hardware.
///
/// **Must be run oversampled.** The polynomial fits assume Open303's
/// oversampled rate, and `new`/`set_rate` want the rate `process` is actually
/// called at — not the engine rate. Its host runs it at 4x. Feed it the engine
/// rate and the tuning is simply wrong, quietly.
#[derive(Debug, Clone)]
pub struct LadderOpen303 {
    y1: f32,
    y2: f32,
    y3: f32,
    y4: f32,
    /// one-pole LP state; the feedback highpass is x - LP(x)
    fb_lp: f32,
    fb_hp_coef: f32,
    sample_rate: f32,
}

impl LadderOpen303 {
    /// `sr` must be the rate `process` is actually called at (oversampled).
    pub fn new(sr: f32) -> Self {
        let mut f = Self {
            y1: 0.0, y2: 0.0, y3: 0.0, y4: 0.0,
            fb_lp: 0.0, fb_hp_coef: 0.0,
            sample_rate: sr,
        };
        f.set_rate(sr);
        f
    }

    /// `sr` must be the rate `process` is actually called at (oversampled).
    pub fn set_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        // 150 Hz feedback highpass (Open303: setFeedbackHighpassCutoff(150.0))
        self.fb_hp_coef = 1.0 - (-2.0 * std::f32::consts::PI * 150.0 / sr).exp();
    }

    /// Process one sample. `cutoff` in Hz, `resonance` 0-1.
    #[inline(always)]
    pub fn process(&mut self, input: f32, cutoff: f32, resonance: f32) -> f32 {
        // resonance skew: (1 - e^(-3r)) / (1 - e^-3)  (Open303 setResonance)
        let r = (1.0 - (-3.0 * resonance.clamp(0.0, 1.0)).exp()) * 1.052_395_3;
        // TB_303-mode coefficient fits (Open303 calculateCoefficientsApprox4):
        // fx = wc * (1/sqrt(2)) / (2*pi)  with wc = 2*pi*fc/fs
        let fx = cutoff / (self.sample_rate * std::f32::consts::SQRT_2);
        let b0 = (0.000_455_223_46 + 6.192_218_9 * fx)
            / (1.0 + 12.358_354 * fx + 4.415_634_5 * fx * fx);
        let mut k = fx * (fx * (fx * (fx * (fx * (fx + 7198.6997) - 5837.7917)
            - 476.47308) + 614.95611) + 213.87126) + 16.998792;
        let mut g = 0.058_823_529 * k;
        g = (g - 1.0) * r + 1.0;
        g *= 1.0 + r;
        k *= r;

        // 150 Hz highpass in the FEEDBACK path: the squelch
        let xfb = k * self.y4;
        self.fb_lp += (xfb - self.fb_lp) * self.fb_hp_coef;
        let y0 = input - (xfb - self.fb_lp);

        // cross-coupled diode chain (verbatim Open303 TB_303 branch)
        self.y1 += 2.0 * b0 * (y0 - self.y1 + self.y2);
        self.y2 += b0 * (self.y1 - 2.0 * self.y2 + self.y3);
        self.y3 += b0 * (self.y2 - 2.0 * self.y3 + self.y4);
        self.y4 += b0 * (self.y3 - 2.0 * self.y4);

        2.0 * g * self.y4
    }

    pub fn reset(&mut self) {
        self.y1 = 0.0;
        self.y2 = 0.0;
        self.y3 = 0.0;
        self.y4 = 0.0;
        self.fb_lp = 0.0;
    }
}

// ─── Multi-mode filter ───────────────────────────────────────────────────────
//
// A voice filter that switches between a 24 dB Moog ladder, a 12 dB SVF (low /
// high-cascade / band / notch), and a resonant comb. Built on the primitives
// above.

/// Selectable response for [`MultiModeFilter`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    LP24,
    LP12,
    HP24,
    BP,
    Notch,
    CombPlus,
    CombMinus,
}

impl FilterMode {
    pub const ALL: &'static [FilterMode] = &[
        FilterMode::LP24,
        FilterMode::LP12,
        FilterMode::HP24,
        FilterMode::BP,
        FilterMode::Notch,
        FilterMode::CombPlus,
        FilterMode::CombMinus,
    ];

    pub fn name(&self) -> &'static str {
        match self {
            FilterMode::LP24 => "LP 24dB",
            FilterMode::LP12 => "LP 12dB",
            FilterMode::HP24 => "HP 24dB",
            FilterMode::BP => "BP",
            FilterMode::Notch => "Notch",
            FilterMode::CombPlus => "Comb+",
            FilterMode::CombMinus => "Comb-",
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => FilterMode::LP24,
            1 => FilterMode::LP12,
            2 => FilterMode::HP24,
            3 => FilterMode::BP,
            4 => FilterMode::Notch,
            5 => FilterMode::CombPlus,
            6 => FilterMode::CombMinus,
            _ => FilterMode::LP24,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            FilterMode::LP24 => 0,
            FilterMode::LP12 => 1,
            FilterMode::HP24 => 2,
            FilterMode::BP => 3,
            FilterMode::Notch => 4,
            FilterMode::CombPlus => 5,
            FilterMode::CombMinus => 6,
        }
    }
}

/// Resonant comb for the Comb+/Comb- modes. Also reused directly as a tunable
/// metallic/Reese comb by engines that want just the comb.
#[derive(Debug, Clone)]
pub struct CombFilter {
    buffer: Vec<f32>,
    write_pos: usize,
    sample_rate: f32,
}

impl CombFilter {
    pub fn new(sample_rate: f32) -> Self {
        let max_delay = (sample_rate * 0.05) as usize; // 50ms max (20Hz)
        Self {
            buffer: vec![0.0; max_delay.max(4)],
            write_pos: 0,
            sample_rate,
        }
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }

    /// `cutoff` tunes the comb (first notch/peak spacing); `feedback` (0..~0.95)
    /// sets resonance; `positive` selects Comb+ (peaks) vs Comb- (notches).
    pub fn process(&mut self, input: f32, cutoff: f32, feedback: f32, positive: bool) -> f32 {
        let freq = cutoff.clamp(20.0, self.sample_rate * 0.45);
        let delay_samples = (self.sample_rate / freq).min(self.buffer.len() as f32 - 1.0).max(1.0);

        // Linear interpolation readback
        let read_pos = self.write_pos as f32 - delay_samples;
        let read_pos = if read_pos < 0.0 {
            read_pos + self.buffer.len() as f32
        } else {
            read_pos
        };
        let idx = read_pos as usize % self.buffer.len();
        let frac = read_pos - read_pos.floor();
        let next = (idx + 1) % self.buffer.len();
        let delayed = self.buffer[idx] * (1.0 - frac) + self.buffer[next] * frac;

        let fb = if positive { feedback } else { -feedback };
        let out = input + delayed * fb;
        self.buffer[self.write_pos] = out.clamp(-4.0, 4.0);
        self.write_pos = (self.write_pos + 1) % self.buffer.len();

        out
    }
}

/// Switchable voice filter: 24 dB ladder / 12 dB SVF taps / resonant comb.
#[derive(Debug, Clone)]
pub struct MultiModeFilter {
    mode: FilterMode,
    ladder: LadderHuovilainen,
    svf1: SvfCytomic,
    svf2: SvfCytomic, // Second SVF for HP24 (cascade two 12dB)
    comb: CombFilter,
}

impl MultiModeFilter {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            mode: FilterMode::LP24,
            ladder: LadderHuovilainen::new(sample_rate),
            svf1: SvfCytomic::new(sample_rate),
            svf2: SvfCytomic::new(sample_rate),
            comb: CombFilter::new(sample_rate),
        }
    }

    pub fn set_mode(&mut self, mode: FilterMode) {
        if self.mode != mode {
            self.mode = mode;
            self.reset();
        }
    }

    pub fn reset(&mut self) {
        self.ladder.reset();
        self.svf1.reset();
        self.svf2.reset();
        self.comb.reset();
    }

    /// Process one sample through the filter.
    pub fn process(&mut self, input: f32, cutoff: f32, resonance: f32) -> f32 {
        match self.mode {
            FilterMode::LP24 => {
                self.ladder.process(input, cutoff, resonance)
            }
            FilterMode::LP12 => {
                let (low, _, _, _) = self.svf1.process(input, cutoff, resonance);
                low
            }
            FilterMode::HP24 => {
                // Cascade two 12dB HP sections
                let (_, high1, _, _) = self.svf1.process(input, cutoff, resonance);
                let (_, high2, _, _) = self.svf2.process(high1, cutoff, resonance);
                high2
            }
            FilterMode::BP => {
                let (_, _, band, _) = self.svf1.process(input, cutoff, resonance);
                band
            }
            FilterMode::Notch => {
                let (_, _, _, notch) = self.svf1.process(input, cutoff, resonance);
                notch
            }
            FilterMode::CombPlus => {
                self.comb.process(input, cutoff, resonance * 0.95, true)
            }
            FilterMode::CombMinus => {
                self.comb.process(input, cutoff, resonance * 0.95, false)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic probe: impulse, then an LCG-noise + sine mix.
    fn probe() -> Vec<f32> {
        let mut sig = Vec::new();
        let mut st: u32 = 12345;
        for i in 0..512 {
            st = st.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = (st >> 8) as f32 / 8388608.0 - 1.0;
            let s = (i as f32 * 0.07).sin();
            sig.push(if i == 0 { 1.0 } else { 0.6 * n + 0.4 * s });
        }
        sig
    }

    fn hash(out: &[f32]) -> u64 {
        out.iter().fold(0u64, |a, &v| a.rotate_left(7) ^ v.to_bits() as u64)
    }

    /// Within 1e-5 of a rendering captured on another machine: libm and FMA
    /// contraction move the last bits, an algorithm change moves far more.
    fn assert_close(got: &[f32], want: &[f32], what: &str) {
        assert_eq!(got.len(), want.len(), "{what}: length");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!((g - w).abs() <= 1e-5, "{what} drifted at sample {i}: {g} vs {w}");
        }
    }

    /// Every 32nd sample of what the five hand-rolled biquads this type
    /// replaced rendered on the probe, captured before they were removed.
    /// A failure means the shared type no longer renders what one of those
    /// engines rendered. Do not re-capture to make it pass.
    #[test]
    fn renders_what_every_legacy_biquad_it_replaced_rendered() {
        let sig = probe();
        let q_butter = std::f32::consts::FRAC_1_SQRT_2;
        let cases: Vec<(&str, Box<dyn Fn(&mut Biquad)>, &[f32])> = vec![
            ("peak", Box::new(move |f: &mut Biquad| f.set_peak(44100.0, 1000.0, 1.2, 6.0)), &[1.0400108, 0.12628692, -0.25309157, -0.024258912, 0.6221907, -0.69537735, 0.24299428, 0.26420474, 0.10072696, 0.301417, -0.20580515, 0.032442734, 1.0776539, -0.03956899, 0.2378884, 0.86671937]),
            ("lowsh", Box::new(move |f: &mut Biquad| f.set_low_shelf(44100.0, 200.0, 0.7, -4.0)), &[0.9953166, 0.18672566, -0.20081258, -0.07482958, 0.5860226, -0.6027822, 0.1634209, 0.3625635, 0.1097126, 0.23911932, -0.10554862, 0.069400325, 0.9226069, 0.02758646, 0.19940108, 0.86695206]),
            ("highsh", Box::new(move |f: &mut Biquad| f.set_high_shelf(44100.0, 6000.0, 0.7, 3.0)), &[1.2792734, 0.37723273, -0.047169633, -0.11582337, 0.72164595, -0.6003472, 0.036606714, 0.5303663, 0.25558957, 0.20352218, 0.09435987, 0.21117046, 1.0479224, 0.18880433, 0.33584812, 1.1129117]),
            ("bp", Box::new(move |f: &mut Biquad| f.set_bandpass(44100.0, 800.0, 2.0)), &[0.027647398, -0.056951847, -0.105126694, 0.14674714, -0.070522875, -0.1086488, 0.20705232, -0.17755066, 0.005661018, 0.13430476, -0.15985756, 0.044982117, 0.13568228, -0.17241924, 0.14424813, -0.060513098]),
            ("lp", Box::new(move |f: &mut Biquad| f.set_lowpass(44100.0, 3000.0, 0.707)), &[0.03478484, 0.182506, -0.20930862, 0.04615266, 0.37231916, -0.44772327, 0.089597836, 0.14505298, -0.36940485, 0.35772127, -0.055424668, -0.33200434, 0.53625643, -0.16382648, -0.122893, 0.26899198]),
            ("lr_lp", Box::new(move |f: &mut Biquad| f.set_lowpass(44100.0, 1200.0, q_butter)), &[0.0065071257, 0.39499483, -0.16045068, 0.036483668, 0.3611601, -0.3877665, -0.064301714, 0.29573834, -0.40271994, 0.21692997, 0.1360728, -0.24713367, 0.35307467, -0.09576467, -0.22843137, 0.37762928]),
            ("lr_hp", Box::new(move |f: &mut Biquad| f.set_highpass(44100.0, 1200.0, q_butter)), &[0.88610566, 0.11263175, 0.17853123, -0.1782023, 0.27208483, -0.04579319, -0.106068864, 0.29554543, 0.48234797, -0.16309932, 0.11534022, 0.3750523, 0.322737, 0.32526147, 0.2716397, 0.5542826]),
        ];
        for (name, cfg, want) in cases {
            let mut f = Biquad::new();
            cfg(&mut f);
            let out: Vec<f32> = sig.iter().map(|&x| f.process(x)).step_by(32).collect();
            assert_close(&out, want, name);
        }
    }

    /// The two hand-rolled `SvfFilter` copies this type replaced produced
    /// identical bits on all four taps, which is what made the merge safe.
    #[test]
    fn renders_what_the_legacy_svf_filters_rendered() {
        let sig = probe();
        let mut f = SvfCytomic::new(44100.0);
        let (mut lo, mut hi, mut bd, mut nt) = (vec![], vec![], vec![], vec![]);
        for &x in &sig {
            let (l, h, b, n) = f.process(x, 1200.0, 0.7);
            lo.push(l); hi.push(h); bd.push(b); nt.push(n);
        }
        let every = |v: &Vec<f32>| v.iter().step_by(32).copied().collect::<Vec<_>>();
        assert_close(&every(&lo), &[0.006935966, 0.4564206, -0.2522547, 0.23881683, 0.34196326, -0.53997743, 0.109462, 0.199973, -0.49694434, 0.37759253, 0.06962504, -0.1749579, 0.5020187, -0.2594809, -0.14909711, 0.3838667], "low tap");
        assert_close(&every(&hi), &[0.944501, 0.0020502806, 0.18174791, -0.32938182, 0.35235092, -0.03892541, -0.13106216, 0.3259852, 0.524033, -0.18241502, 0.021035857, 0.25436112, 0.35815918, 0.37417185, 0.20305926, 0.55744827], "high tap");
        assert_close(&every(&bd), &[0.080938414, -0.3159066, -0.1608761, -0.014140976, -0.052222896, -0.06827374, 0.18630876, -0.19122814, 0.08113289, 0.053967107, -0.25501725, -0.08200911, 0.15471126, -0.09046434, 0.13704947, -0.04578743], "band tap");
        assert_close(&every(&nt), &[0.95143694, 0.45847088, -0.070506774, -0.09056498, 0.6943142, -0.57890284, -0.021600164, 0.5259582, 0.027088646, 0.19517751, 0.0906609, 0.07940322, 0.8601779, 0.11469095, 0.053962134, 0.941315], "notch tap");
    }

    /// The cutoff sweeps on purpose so the coefficient cache thresholds
    /// (0.5 Hz / 0.001) are part of what is locked down.
    #[test]
    fn renders_what_the_legacy_tpt_svfs_rendered() {
        let sig = probe();
        let mut f = SvfTpt::new(44100.0);
        let (mut lp, mut hp, mut bp) = (vec![], vec![], vec![]);
        for (i, &x) in sig.iter().enumerate() {
            let (l, h, b) = f.tick_at(x, 400.0 + (i as f32 * 0.9), 0.6);
            lp.push(l); hp.push(h); bp.push(b);
        }
        let every = |v: &Vec<f32>| v.iter().step_by(32).copied().collect::<Vec<_>>();
        assert_close(&every(&lp), &[0.0008101876, 0.43358597, 0.21423642, -0.9010597, 1.3102368, -0.6467355, -0.9589037, 2.2219644, -2.7073295, 2.4002059, -1.6387364, 1.1045909, -0.7342624, 0.7592042, -0.9676395, 0.80609417], "lp tap");
        assert_close(&every(&hp), &[0.9972588, -0.18585703, -0.33523518, 0.7826682, -0.69717556, 0.1467969, 0.91822183, -1.7297693, 2.7840707, -2.2397926, 1.6637154, -1.1429467, 1.7009273, -0.6642317, 1.0338104, 0.19288445], "hp tap");
        assert_close(&every(&bp), &[0.028424755, 0.31203434, -0.67761576, 0.28471404, 0.7348108, -1.7653472, 1.9263599, -1.1919351, -0.014317602, 0.9883648, -1.2854753, 1.0091078, -0.20107797, -0.5087253, 1.0307078, -1.2532032], "bp tap");
    }

    /// Every 32nd sample of each ladder's rendering of the probe, captured
    /// from the hand-rolled ladders the engines carried before they moved
    /// here. Compared within a tolerance: the ladders go through `tanh`,
    /// whose last bits belong to the libm of the machine.
    #[test]
    fn ladders_render_what_the_engines_they_came_from_rendered() {
        let sig = probe();
        let sweep = |f: &mut dyn FnMut(f32, f32) -> f32| -> Vec<f32> {
            sig.iter().enumerate().map(|(i, &x)| f(x, 400.0 + i as f32 * 0.9)).step_by(32).collect()
        };

        let mut b = LadderCascadeTanh::new(44100.0);
        assert_close(&sweep(&mut |x, co| b.process(x, co, 0.6)), &[5.898387e-7, 0.005397107, 0.04109183, 0.04561678, -0.014757508, -0.013384979, -0.018684678, -0.032918457, 0.06196469, -0.0002375464, -0.06354362, 0.0441619, 0.0045779655, 0.007724733, -0.011619087, -0.0576302], "bass ladder");

        let mut v = LadderCascadeDiode::new(44100.0);
        assert_close(&sweep(&mut |x, co| v.process(x, co, 0.6)), &[4.537221e-7, 0.00501698, 0.04080976, 0.05382268, 0.0150555, 0.020058826, -0.019600384, -0.06888378, 0.017784866, 0.011455083, -0.0021488285, 0.02693311, -0.045323815, 0.028928574, 0.02382211, -0.07195729], "vcs3 ladder");

        let mut n = LadderHuovilainen::new(44100.0);
        assert_close(&sweep(&mut |x, co| n.process(x, co, 0.6)), &[9.531569e-9, 0.00014607284, 0.0019305191, 0.0061088526, 0.010611992, 0.016481265, 0.021635838, 0.019916035, 0.015410306, 0.007876327, -0.0032458296, -0.009311605, -0.013871084, -0.013841298, -0.005013365, -0.0018464272], "polaris ladder");

        let mut m = LadderTpt::new(44100.0);
        assert_close(&sweep(&mut |x, co| m.process(x, co, 0.6, 0.3)), &[1.1280407e-6, 0.09523123, 0.34547636, -0.2746078, -0.0040467232, 0.50207597, -0.8261334, 0.3749966, 0.30699256, -0.8043124, 0.8944632, -0.6205707, 0.14831652, 0.35562018, -0.72252244, 0.75297725], "magma ladder");

        // 4x rate, the way its host actually drives it.
        let mut t = LadderOpen303::new(44100.0 * 4.0);
        assert_close(&sweep(&mut |x, co| t.process(x, co, 0.6)), &[8.219284e-8, 0.0009123949, 0.009300957, 0.021673242, 0.027422078, 0.036110193, 0.03349236, 0.008724244, -0.0040369527, -0.01559932, -0.017829223, 0.006487145, 0.016631957, 0.029005608, 0.033565175, -0.0018633145], "tb303 ladder");
    }

    /// The ladders are five different filters, not five copies. If any two of
    /// these ever agree, one of them has been "unified" into the other and an
    /// engine has lost its voice.
    #[test]
    fn the_ladders_are_all_different_filters() {
        let sig = probe();
        let mut hashes = Vec::new();

        let mut b = LadderCascadeTanh::new(44100.0);
        hashes.push(hash(&sig.iter().map(|&x| b.process(x, 900.0, 0.6)).collect::<Vec<_>>()));
        let mut v = LadderCascadeDiode::new(44100.0);
        hashes.push(hash(&sig.iter().map(|&x| v.process(x, 900.0, 0.6)).collect::<Vec<_>>()));
        let mut n = LadderHuovilainen::new(44100.0);
        hashes.push(hash(&sig.iter().map(|&x| n.process(x, 900.0, 0.6)).collect::<Vec<_>>()));
        let mut m = LadderTpt::new(44100.0);
        hashes.push(hash(&sig.iter().map(|&x| m.process(x, 900.0, 0.6, 0.0)).collect::<Vec<_>>()));
        let mut t = LadderOpen303::new(44100.0 * 4.0);
        hashes.push(hash(&sig.iter().map(|&x| t.process(x, 900.0, 0.6)).collect::<Vec<_>>()));

        let mut uniq = hashes.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), hashes.len(), "two ladders collapsed into the same filter");
    }

    /// Every 250th sample of each mode's rendering, captured from the
    /// hand-rolled multimode filter before it moved here. Locks the
    /// composite that two instruments drive.
    #[test]
    fn multimode_renders_what_the_legacy_filter_rendered() {
        let golden: [[f32; 16]; 7] = [
            [2.8799113e-10, 0.007753386, -0.0022069386, 0.037814382, 0.0035401275, -0.013962893, 0.023952732, 0.06998324, 0.0028163893, 0.04295335, -0.042200428, 0.0029640347, -0.024722692, 0.06839536, -0.08455859, -0.051678605],
            [3.354249e-5, 0.014785105, 0.067030214, -0.14744219, 0.025368774, 0.16609044, 0.10594829, 0.21106493, 0.50640637, 0.3655322, -0.65838504, 0.019398633, -0.3535173, -0.2360467, -0.6267249, -0.1225497],
            [0.07245489, -0.07230295, -0.47880405, 0.19341545, 1.0780425, 0.22661531, 1.1691554, -0.9266039, -0.21046993, 0.1821363, 0.30501077, -1.2003081, 0.90182644, -0.9414033, 1.0924478, -0.49245936],
            [0.0015692661, 0.049025998, 0.31549475, -0.36699492, -0.008042052, 0.25726503, -0.47675318, 0.69142723, -0.14904806, 0.24145518, -0.1282239, 0.19230273, -0.67041403, 0.73332685, 0.06258032, 0.3499419],
            [0.07345076, -0.11141323, -0.17699851, -0.11857772, 0.97384846, 0.8086296, 0.2866189, 0.051049083, 0.23518509, 0.7426436, -0.11486387, -1.0480922, 0.06589538, 0.052131772, 0.58640945, -0.12831204],
            [0.07439232, 0.14064927, -0.7917721, -0.34195277, 0.7130345, 1.0960569, -0.44019648, 0.9778468, 0.5169718, 1.5081159, -0.48769993, -0.28265405, -0.45605844, 0.871434, 0.03980297, -0.2542668],
            [0.07439232, -0.21326791, 0.4683779, -0.3896219, 0.9168956, 0.9343756, 0.73009294, 0.18740174, -0.44131565, 0.70944536, 0.18835482, -1.0942538, -0.1557748, 0.6995521, 0.44298905, 0.6891127],
        ];
        for (m, want) in golden.iter().enumerate() {
            let mut f = MultiModeFilter::new(44100.0);
            f.set_mode(FilterMode::from_index(m));
            let mut st: u32 = 777;
            let mut out = Vec::new();
            for i in 0..4000 {
                st = st.wrapping_mul(1664525).wrapping_add(1013904223);
                let x = (st >> 9) as f32 / 4194304.0 - 1.0;
                out.push(f.process(x, 300.0 + i as f32 * 1.2, 0.7));
            }
            let got: Vec<f32> = out.into_iter().step_by(250).collect();
            assert_close(&got, want, &format!("multimode filter mode {m}"));
        }
    }

    /// The two SVF topologies are algebraically equal but must never be swapped
    /// for one another. If this ever passes, someone unified them and silently
    /// re-rendered half the catalogue.
    #[test]
    fn the_two_svf_forms_are_not_interchangeable() {
        let sig = probe();
        let mut c = SvfCytomic::new(44100.0);
        let mut t = SvfTpt::new(44100.0);
        let a: Vec<f32> = sig.iter().map(|&x| c.process(x, 1200.0, 0.7).0).collect();
        let b: Vec<f32> = sig.iter().map(|&x| t.tick_at(x, 1200.0, 0.7).0).collect();
        assert_ne!(hash(&a), hash(&b));
    }
}
