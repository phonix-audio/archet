//! Modal bowed string -- Demoucron 2008 (PhD, IRCAM / KTH "On the control of
//! virtual violins"), Chapter 2. This REPLACES the digital-waveguide string.
//!
//! Why modal, not waveguide: the string is a bank of N independent damped
//! harmonic oscillators (modes). Each mode has its OWN frequency (inharmonic via
//! stiffness) AND its OWN decay rate r_n = B1 + B2*(n-1)^2 (Adrien's law,
//! Eq. 2.13). That PER-PARTIAL decay control -- a long-ringing fundamental with
//! fast-decaying highs -- is exactly the character of a real cello/bass and is
//! precisely what a single waveguide loss filter cannot reproduce. Demoucron uses
//! the SAME static hyperbolic friction curve as the STK/MSW models; the realism
//! is in the modal string + true bridge-force output, not the friction.
//!
//! Per sample, each 2nd-order modal ODE is integrated by its EXACT analytical
//! solution assuming the bow force is constant across the timestep (Eqs. 2.17-2.24
//! -- this preserves the stick<->slip force discontinuity). The bow friction is
//! the intersection of the hyperbolic curve with the string's numerical-impedance
//! response line (Friedlander / McIntyre-Schumacher-Woodhouse, modal form,
//! Eqs. 2.29-2.34). Output is the bridge FORCE (Eq. 2.35), not a displacement tap.

pub const MAX_MODES: usize = 90;

#[derive(Clone, Debug)]
pub struct ModalString {
    sr: f32,
    dt: f32,
    n: usize,
    // precomputed per-mode integration coefficients (Eqs. 2.17-2.24). f64: the
    // high-Q (low-damping) modes sit near the f32 stability edge, and v0h (a sum of
    // ~90 modal velocities) needs the precision so the stick/slip decision is stable
    // cycle-to-cycle -- f32 roundoff there jittered the slip timing into broadband
    // noise (the "white noise" at low damping).
    x1: Vec<f64>,
    x2: Vec<f64>,
    x3: Vec<f64>,
    y1: Vec<f64>,
    y2: Vec<f64>,
    y3: Vec<f64>,
    phi0: Vec<f64>,     // phi_n(x0): modal shape at the bow point
    // phi0*x3 and phi0*y3 are constant per note but were multiplied out every
    // sample in the bow hot loop; precompute them so process()'s second loop
    // streams one fewer constant array and does 2 fewer f64 muls per mode per
    // sample (this modal bank is memory-bandwidth bound).
    // (phi0[i]*x3[i])*f0 == phi0_x3[i]*f0 bit-for-bit (left-assoc f64 mul).
    phi0_x3: Vec<f64>,
    phi0_y3: Vec<f64>,
    wbridge: Vec<f64>,  // bridge-force weight per mode (~n + stiffness)
    // per-mode state: modal displacement a_n and velocity a_n-dot
    a: Vec<f64>,
    adot: Vec<f64>,
    b00: f64,           // numerical admittance  sum phi0^2 * y3  (C01 = 1/b00)
    // friction characteristic (hyperbolic, Eq. 2.33)
    mu_s: f32,
    mu_d: f32,
    v0: f32,            // friction-curve velocity scale
    pub beta: f32,      // bow-bridge fraction x0/L
    pub noise_amt: f32, // slip noise amplitude (Demoucron Eq. for N(t)=1-A*u)
    pub release_damp: f32, // per-sample extra state decay on bow-off (1.0 = natural ring)
    rng: u32,
    pub slipping: bool,
    // ── string/fret unilateral CONTACT (slap bass, IRCAM Modalys style) ──────
    // A barrier placed at string fraction fret_beta. When the vibrating string
    // penetrates it, a clamp force is applied across ALL modes (coupling them -> the
    // buzzy, slightly inharmonic "growl" that colours the WHOLE slap note, not a gated
    // attack frise). The barrier is RELATIVE to the string's own running swing
    // (scale-independent: modal displacements are tiny, force-dependent units), and the
    // force is NORMALIZED so fret_hard in [0,1] is a real clamp fraction (1 = rigid fret).
    fret_phi:  Vec<f64>,  // modal shape at the contact point
    fret_frac: f64,       // barrier at -fret_frac * swing (smaller -> deeper into the swing -> more growl)
    fret_hard: f64,       // clamp hardness 0..1
    fret_norm: f64,       // 1 / sum(fret_phi^2 * x3): converts a desired displacement correction to a force
    disp_env:  f64,       // running envelope of |displacement at the fret|
    fret_on:   bool,
}

impl ModalString {
    pub fn new(sr: f32) -> Self {
        let mut s = Self {
            sr,
            dt: 1.0 / sr,
            n: 0,
            x1: vec![0.0; MAX_MODES], x2: vec![0.0; MAX_MODES], x3: vec![0.0; MAX_MODES],
            y1: vec![0.0; MAX_MODES], y2: vec![0.0; MAX_MODES], y3: vec![0.0; MAX_MODES],
            phi0: vec![0.0; MAX_MODES],
            phi0_x3: vec![0.0; MAX_MODES], phi0_y3: vec![0.0; MAX_MODES],
            wbridge: vec![0.0; MAX_MODES],
            a: vec![0.0; MAX_MODES], adot: vec![0.0; MAX_MODES],
            b00: 1.0_f64,
            mu_s: 0.8, mu_d: 0.3, v0: 0.10,
            beta: 0.10, noise_amt: 0.0, release_damp: 1.0, rng: 0x1234_5678, slipping: false,
            fret_phi: vec![0.0; MAX_MODES], fret_frac: 0.5, fret_hard: 0.0,
            fret_norm: 1.0, disp_env: 0.0, fret_on: false,
        };
        s.set_voice(196.0, 3.0, 6.0, 1.0e-4, 0.10);
        s
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    /// Configure the string for a note. `f0` Hz; `b1`,`b2` modal-damping law
    /// r_n = b1 + b2*(n-1)^2 (b2 = how fast high partials die -> darker); `stiff`
    /// dimensionless inharmonicity; `beta` bow position (x0/L).
    pub fn set_voice(&mut self, f0: f32, b1: f32, b2: f32, stiff: f32, beta: f32) {
        self.beta = beta.clamp(0.02, 0.25);
        // cap the modes by a max frequency (fewer high modes -> less chaotic ringing)
        // AND by MAX_MODES. Demoucron-ish default ~ sr/4.
        // Mode-frequency cap: high enough that even high notes get enough modes for
        // clean Helmholtz (a 1.8 kHz A6 at 9 kHz had only ~5 modes -> noise), but the
        // per-note count is still bounded by MAX_MODES.
        let fmax = 16000.0f32;
        let dpow = 2.0f32; // damping growth exponent r_n = b1 + b2*(n-1)^dpow
        // Steep extra damping above mode `ncut` (radiation/air damping) -- suppresses
        // the chaotic ringing of the high modes WITHOUT darkening the bright mid band.
        let b3 = 0.0f32;
        let ncut = 14.0f32;
        let nmax = ((fmax / f0).floor() as usize).min((0.45 * self.sr / f0).floor() as usize);
        self.n = nmax.clamp(4, MAX_MODES);
        let dt = self.dt as f64;
        let (f0, b1, b2, stiff, b3, ncut, beta) =
            (f0 as f64, b1 as f64, b2 as f64, stiff as f64, b3 as f64, ncut as f64, self.beta as f64);
        let dpow = dpow as f64;
        let mut b00 = 0.0f64;
        for i in 0..self.n {
            let k = (i + 1) as f64;
            // inharmonic mode frequency (stiff string), rad/s
            let w0 = 2.0 * std::f64::consts::PI * f0 * k * (1.0 + stiff * k * k).sqrt();
            let over = (k - ncut).max(0.0);
            let rn = b1 + b2 * (k - 1.0).powf(dpow) + b3 * over * over * over;
            let wn2 = (w0 * w0 - rn * rn).max(1.0);
            let wn = wn2.sqrt();
            let theta = wn * dt;
            let rr = (-rn * dt).exp();
            let (st, ct) = theta.sin_cos();
            self.x1[i] = (ct + (rn / wn) * st) * rr;
            self.x2[i] = (st / wn) * rr;
            self.x3[i] = (1.0 - self.x1[i]) / (w0 * w0); // rho_L = 1
            self.y1[i] = -(wn + rn * rn / wn) * st * rr;
            self.y2[i] = (ct - (rn / wn) * st) * rr;
            self.y3[i] = -self.y1[i] / (w0 * w0);
            // modal shape at the bow point (orthonormal sin basis, L=1)
            self.phi0[i] = std::f64::consts::SQRT_2 * (k * std::f64::consts::PI * beta).sin();
            // bridge force ~ T0*dy/dx + EI*d3y/dx3 -> weight ~ k (+ small stiffness k^3)
            self.wbridge[i] = k * (1.0 + stiff * 0.5 * k * k);
            // Same operands/order as the bow hot loop -> bit-identical there.
            self.phi0_x3[i] = self.phi0[i] * self.x3[i];
            self.phi0_y3[i] = self.phi0[i] * self.y3[i];
            b00 += self.phi0[i] * self.phi0[i] * self.y3[i];
        }
        self.b00 = if b00.abs() < 1e-12 { 1e-12 } else { b00 };
    }

    /// Configure with an explicit per-partial T60 law (Välimäki 2004: harpsichord
    /// partial decays are NON-monotonic -- a one-pole trend with a RIPPLE so
    /// neighboring partials differ; the 2nd partial often outlives the 1st).
    /// `t60(k)` returns the decay time (s) of partial k (1-based).
    pub fn set_voice_t60(&mut self, f0: f32, stiff: f32, beta: f32,
                         t60: &dyn Fn(usize) -> f32) {
        let beta = beta.clamp(0.02, 0.55);
        self.beta = beta;
        let fmax = 16000.0f32;
        let nmax = ((fmax / f0).floor() as usize).min((0.45 * self.sr / f0).floor() as usize);
        self.n = nmax.clamp(4, MAX_MODES);
        let dt = self.dt as f64;
        let (f0d, stiffd, betad) = (f0 as f64, stiff as f64, beta as f64);
        let mut b00 = 0.0f64;
        for i in 0..self.n {
            let k = (i + 1) as f64;
            let w0 = 2.0 * std::f64::consts::PI * f0d * k * (1.0 + stiffd * k * k).sqrt();
            let rn = (6.9078 / t60(i + 1).max(0.05)) as f64;
            let wn2 = (w0 * w0 - rn * rn).max(1.0);
            let wn = wn2.sqrt();
            let theta = wn * dt;
            let rr = (-rn * dt).exp();
            let (st, ct) = theta.sin_cos();
            self.x1[i] = (ct + (rn / wn) * st) * rr;
            self.x2[i] = (st / wn) * rr;
            self.x3[i] = (1.0 - self.x1[i]) / (w0 * w0);
            self.y1[i] = -(wn + rn * rn / wn) * st * rr;
            self.y2[i] = (ct - (rn / wn) * st) * rr;
            self.y3[i] = -self.y1[i] / (w0 * w0);
            self.phi0[i] = std::f64::consts::SQRT_2 * (k * std::f64::consts::PI * betad).sin();
            self.wbridge[i] = k * (1.0 + stiffd * 0.5 * k * k);
            self.phi0_x3[i] = self.phi0[i] * self.x3[i];
            self.phi0_y3[i] = self.phi0[i] * self.y3[i];
            b00 += self.phi0[i] * self.phi0[i] * self.y3[i];
        }
        self.b00 = if b00.abs() < 1e-12 { 1e-12 } else { b00 };
    }

    /// Numerical impedance C01 = 1/b00 (the modal analogue of 2*Zc). The bow force
    /// must be scaled to this for the string to STICK (clean Helmholtz).
    pub fn impedance(&self) -> f32 {
        (1.0 / self.b00) as f32
    }

    /// PLUCK: one-shot force injection at the excitation point (beta). A plectrum
    /// pluck is a displacement step at the pluck point -- exactly one application of
    /// the per-mode force add, then a free linear ring-down with the per-partial
    /// decay. (The harpsichord case: the EASY case of the modal string.)
    pub fn excite(&mut self, force: f32) {
        self.excite_tilt(force, 0.0);
    }

    /// Pluck through a PLECTRUM-COMPLIANCE lowpass: per-mode force weighted by
    /// 1/(1+(f_k/fc)^2). An ideal force step is infinitely sharp; a real plectrum's
    /// finite width/compliance rolls the excitation off (~4 kHz on a harpsichord).
    pub fn excite_lp(&mut self, force: f32, fc_hz: f32, f0: f32) {
        let f = force as f64;
        let fc = fc_hz.max(200.0) as f64;
        let f0d = f0 as f64;
        for i in 0..self.n {
            let fk = f0d * (i + 1) as f64;
            let w = 1.0 / (1.0 + (fk / fc) * (fk / fc));
            self.a[i] += self.phi0[i] * self.x3[i] * f * w;
            self.adot[i] += self.phi0[i] * self.y3[i] * f * w;
        }
    }

    /// Pluck with a BRIGHTNESS tilt: per-mode force weighted by k^tilt. A plain
    /// impulse gives mode energy ~1/k^2 (dark); a harpsichord plectrum snap is far
    /// brighter -- tilt ~1 makes the bridge spectrum ~flat (FluidR3 fit: centroid
    /// ~4 kHz at EVERY pitch, even f0=73 Hz).
    pub fn excite_tilt(&mut self, force: f32, tilt: f32) {
        let f = force as f64;
        let t = tilt as f64;
        for i in 0..self.n {
            let w = ((i + 1) as f64).powf(t);
            self.a[i] += self.phi0[i] * self.x3[i] * f * w;
            self.adot[i] += self.phi0[i] * self.y3[i] * f * w;
        }
    }

    /// Partially damp the existing modal energy (for legato retune): the old pitch's
    /// modal phases don't form the NEW pitch's Helmholtz corner, so they beat ("weird"
    /// slur) until the bow re-locks. Knocking the stale energy down lets the (still
    /// bowing) string establish the new corner faster, without a silent gap.
    pub fn soften(&mut self, factor: f32) {
        let f = factor as f64;
        for i in 0..self.n {
            self.a[i] *= f;
            self.adot[i] *= f;
        }
    }

    /// Reset all modal energy (silence). Use for a fresh note attack.
    pub fn reset(&mut self) {
        for i in 0..self.n {
            self.a[i] = 0.0;
            self.adot[i] = 0.0;
        }
        self.slipping = false;
    }

    /// Arm the string/fret unilateral contact (slap bass). `beta_fret` = contact
    /// position (fraction of string length, near the neck for a thumb slap); `frac` =
    /// barrier depth as a fraction of the string's own swing (smaller -> more growl);
    /// `hard` = clamp hardness 0..1 (1 = rigid fret). Call after `set_voice`. `hard<=0`
    /// disables it. Scale-independent: the barrier tracks the live amplitude.
    pub fn set_fret(&mut self, beta_fret: f32, frac: f32, hard: f32) {
        let b = (beta_fret as f64).clamp(0.02, 0.5);
        let mut s = 0.0f64;
        for i in 0..self.n {
            self.fret_phi[i] = std::f64::consts::SQRT_2 * ((i + 1) as f64 * std::f64::consts::PI * b).sin();
            s += self.fret_phi[i] * self.fret_phi[i] * self.x3[i];
        }
        self.fret_norm = 1.0 / s.max(1e-30);  // force that produces unit displacement at the fret
        self.fret_frac = (frac as f64).clamp(0.05, 0.98);
        self.fret_hard = (hard as f64).clamp(0.0, 1.0);
        self.disp_env = 0.0;
        self.fret_on = hard > 0.0;
    }

    pub fn clear_fret(&mut self) { self.fret_on = false; self.fret_hard = 0.0; }

    /// The bridge force the string would output right now (no state advance). Used to
    /// NORMALIZE a pluck across pitch: low notes have many in-phase modes -> a far larger
    /// attack sum; scaling the post-excite state by target/bridge_now() makes the attack
    /// level pitch-independent (and clip-free) without touching the per-partial decay.
    pub fn bridge_now(&self) -> f32 {
        let mut b = 0.0f64;
        for i in 0..self.n { b += self.a[i] * self.wbridge[i]; }
        b as f32
    }

    /// One sample of a freely-ringing (plucked) string WITH the string/fret contact.
    /// No bow. Returns the bridge force. When the displacement at the fret penetrates
    /// the (amplitude-relative) barrier, a normalized clamp force pushes it back across
    /// all modes -> the buzzy, slightly inharmonic slap growl that pervades the whole
    /// note (the contact recurs every cycle while the string swings, then fades with it).
    #[inline]
    pub fn process_slap(&mut self) -> f32 {
        let n = self.n;
        // free ("historical") update of every mode
        {
            let a = &mut self.a[..n];
            let adot = &mut self.adot[..n];
            let (x1, x2, y1, y2) = (&self.x1[..n], &self.x2[..n], &self.y1[..n], &self.y2[..n]);
            for i in 0..n {
                let ah = x1[i] * a[i] + x2[i] * adot[i];
                let bh = y1[i] * a[i] + y2[i] * adot[i];
                a[i] = ah;
                adot[i] = bh;
            }
        }
        // unilateral contact at the fret point, barrier relative to the live swing
        let mut f0 = 0.0f64;
        if self.fret_on {
            let mut disp = 0.0f64;
            {
                let a = &self.a[..n];
                let fp = &self.fret_phi[..n];
                for i in 0..n { disp += a[i] * fp[i]; }
            }
            self.disp_env += (disp.abs() - self.disp_env) * 0.02; // ~ couple-ms swing tracker
            let barrier = self.fret_frac * self.disp_env;
            if disp < -barrier {
                let pen = disp + barrier;                  // <0, the penetration
                f0 = -self.fret_hard * pen * self.fret_norm; // clamp force pushing the string off the fret
            }
        }
        // apply contact force + read out the bridge force
        let mut bridge = 0.0f64;
        {
            let a = &mut self.a[..n];
            let adot = &mut self.adot[..n];
            let (fp, x3, y3, wb) = (&self.fret_phi[..n], &self.x3[..n], &self.y3[..n], &self.wbridge[..n]);
            for i in 0..n {
                a[i] += fp[i] * x3[i] * f0;
                adot[i] += fp[i] * y3[i] * f0;
                bridge += a[i] * wb[i];
            }
        }
        bridge as f32
    }

    /// One sample. `bow_vel` (m/s-ish), `bow_force` (>0 = pressing). Returns the
    /// bridge force (the radiated signal, pre-body).
    #[inline]
    pub fn process(&mut self, bow_vel: f32, bow_force: f32) -> f32 {
        // --- free ("historical") update of every mode + bow-point velocity ---
        // ALL f64: v0h is a sum of ~90 modal velocities; f32 roundoff here flipped the
        // stick/slip decision cycle-to-cycle and jittered the slip into white noise.
        let mut v0h = 0.0f64;
        let n = self.n;
        // Bind [..n] slices so the per-sample loop drops bounds checks and
        // autovectorizes. SAME ops in the SAME order -> bit-identical output.
        {
            let a = &mut self.a[..n];
            let adot = &mut self.adot[..n];
            let (x1, x2, y1, y2, phi0) =
                (&self.x1[..n], &self.x2[..n], &self.y1[..n], &self.y2[..n], &self.phi0[..n]);
            for i in 0..n {
                let ah = x1[i] * a[i] + x2[i] * adot[i];
                let bh = y1[i] * a[i] + y2[i] * adot[i];
                a[i] = ah; // historical; force term added below
                adot[i] = bh;
                v0h += phi0[i] * bh;
            }
        }

        // --- friction interaction (Eqs. 2.30-2.34), no finger force ---
        let c01 = 1.0 / self.b00;
        let fb = bow_force.max(0.0) as f64;
        let vb = bow_vel as f64;
        let mu_s = self.mu_s as f64;
        let mu_d = self.mu_d as f64;
        let f0_stick = c01 * (vb - v0h); // force to make string velocity = bow (stick)
        let mut f0;
        if fb <= 0.0 {
            // bow fully lifted / free-ringing pluck: exactly force-free (the slip
            // quadratic can return spurious force at fb=0), and cheaper on tails.
            f0 = 0.0;
            self.slipping = false;
        } else if f0_stick.abs() <= mu_s * fb {
            f0 = f0_stick; // sticking
            self.slipping = false;
        } else {
            // slipping: solve the quadratic in dv = ydot(x0) - vb
            let sgn = if vb >= 0.0 { 1.0 } else { -1.0 };
            let vc = self.v0 as f64 * sgn;
            let fc = fb * sgn;
            let h = vb - v0h;
            let c2 = -c01 / vc;
            let c1 = c01 * (1.0 - h / vc) + mu_d * fb / vc;
            let c0 = c01 * h - mu_s * fc;
            let disc = c1 * c1 - 4.0 * c0 * c2;
            if disc < 0.0 {
                f0 = f0_stick; // no slip solution -> sticks
                self.slipping = false;
            } else {
                let dv = (-c1 + disc.sqrt()) / (2.0 * c2);
                if dv * vb > 0.0 {
                    f0 = f0_stick; // solution on the wrong branch -> sticks
                    self.slipping = false;
                } else {
                    f0 = c01 * (dv + vb - v0h);
                    self.slipping = true;
                }
            }
        }
        // slip noise: friction is roughened during slip (Demoucron, N(t)=1-A*u)
        if self.slipping && self.noise_amt > 0.0 {
            let u = (self.rand() * 0.5 + 0.5).clamp(0.0, 1.0); // [0,1]
            f0 *= 1.0 - (self.noise_amt * u) as f64;
        }

        // --- apply the bow force to every mode, read out the bridge force ---
        // On bow-off (release_damp < 1) add extra per-sample decay so a detache note
        // settles in ~150 ms instead of ringing ~500 ms like a plucked harp/harpsichord.
        let rd = self.release_damp as f64;
        let mut bridge = 0.0f64;
        let n = self.n;
        {
            let a = &mut self.a[..n];
            let adot = &mut self.adot[..n];
            // phi0_x3 = phi0*x3, phi0_y3 = phi0*y3 (precomputed in set_voice): two
            // fewer constant arrays streamed per sample on this bandwidth-bound bank.
            let (phi0_x3, phi0_y3, wbridge) =
                (&self.phi0_x3[..n], &self.phi0_y3[..n], &self.wbridge[..n]);
            for i in 0..n {
                a[i] = (a[i] + phi0_x3[i] * f0) * rd;
                adot[i] = (adot[i] + phi0_y3[i] * f0) * rd;
                bridge += a[i] * wbridge[i];
            }
        }
        bridge as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locks the bowed modal integration bit-for-bit so hot-loop refactors
    /// (e.g. precomputing phi0*x3 / phi0*y3 out of process()) are provably
    /// output-preserving. noise_amt = 0 keeps it fully deterministic (no rand).
    #[test]
    fn process_is_byte_stable() {
        let mut s = ModalString::new(48_000.0);
        s.set_voice(196.0, 3.0, 6.0, 1.0e-4, 0.10);
        s.noise_amt = 0.0;
        let mut h = 0u64;
        for i in 0..8000 {
            // A slow bow gesture that crosses stick <-> slip and a release tail.
            let vb = (i as f32 * 0.0013).sin() * 0.25;
            let fb = if i < 6000 { 0.3 } else { 0.0 };
            let out = s.process(vb, fb);
            h = h.rotate_left(7) ^ out.to_bits() as u64;
        }
        assert_eq!(h, 2615062553043384055u64, "archet ModalString::process drifted");
    }

    fn render(f0: f32, b1: f32, b2: f32, stiff: f32, beta: f32, vb: f32, fb: f32,
              secs: f32) -> (Vec<f32>, f32) {
        let sr = 48000.0;
        // OVERSAMPLE: run the string at os*sr so the slip time is finely resolved
        // (sample-quantized slip jitter decoheres the short-period high modes).
        let os: usize = 1;
        let mut s = ModalString::new(sr * os as f32);
        s.set_voice(f0, b1, b2, stiff, beta);
        s.noise_amt = 0.0; // measure the CLEAN tone; slip noise added later for realism
        let nframes = (sr * secs) as usize;
        let mut out = vec![0.0f32; nframes];
        // light bridge highpass to remove DC drift + a soft saturator-free output
        let mut hp = 0.0f32;
        let mut dec = 0.0f32; // decimation lowpass state
        let alpha = 1.0 - (-2.0 * std::f32::consts::PI * 16000.0 / (sr * os as f32)).exp();
        for i in 0..nframes {
            let mut raw = 0.0f32;
            for _ in 0..os {
                let r = s.process(vb, fb);
                dec += (r - dec) * alpha; // anti-alias before decimating
                raw = dec;
            }
            hp += (raw - hp) * (1.0 - (-2.0 * std::f32::consts::PI * 20.0 / sr).exp());
            out[i] = raw - hp; // 20 Hz highpass
        }
        // normalize
        let pk = out.iter().fold(0.0f32, |m, &x| m.max(x.abs())).max(1e-9);
        for x in out.iter_mut() { *x /= pk; }
        (out, sr)
    }

    fn centroid(out: &[f32], sr: f32, f0: f32) -> f32 {
        // steady window
        let s0 = (sr * 0.5) as usize;
        let s1 = (s0 + (sr * 0.8) as usize).min(out.len());
        if s1 <= s0 + 1024 { return 0.0; }
        let seg = &out[s0..s1];
        let n = seg.len();
        let mut re = vec![0.0f32; n];
        for (i, w) in re.iter_mut().enumerate() {
            let win = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            *w = seg[i] * win;
        }
        // naive DFT over harmonic bins up to 40*f0
        let mut num = 0.0f64; let mut den = 0.0f64;
        let mut f = f0;
        while f < 8000.0 {
            let mut sr_ = 0.0f64; let mut si = 0.0f64;
            let w = 2.0 * std::f64::consts::PI * f as f64 / sr as f64;
            for (i, &x) in re.iter().enumerate() {
                sr_ += x as f64 * (w * i as f64).cos();
                si -= x as f64 * (w * i as f64).sin();
            }
            let mag = (sr_ * sr_ + si * si).sqrt();
            num += f as f64 * mag; den += mag;
            f += f0;
        }
        (num / den.max(1e-12)) as f32
    }

    fn write_wav(path: &str, out: &[f32], sr: f32) {
        let mut bytes = Vec::new();
        let data_len = out.len() * 2;
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&(sr as u32).to_le_bytes());
        bytes.extend_from_slice(&((sr as u32) * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data_len as u32).to_le_bytes());
        for &x in out {
            let v = (x.clamp(-1.0, 1.0) * 32767.0) as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }

    /// harmonicity = median(harmonic peak) / median(inter-harmonic floor).
    /// Clean bowed tone >> 20; chaotic-slip noise ~ 1-3.
    fn harmonicity(out: &[f32], sr: f32, f0: f32) -> f32 {
        let s0 = (sr * 0.6) as usize;
        let n = 8192.min(out.len() - s0);
        if n < 2048 { return 0.0; }
        let seg = &out[s0..s0 + n];
        let mut re = vec![0.0f32; n];
        for (i, w) in re.iter_mut().enumerate() {
            let win = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            *w = seg[i] * win;
        }
        let dft = |f: f32| -> f32 {
            let mut sr_ = 0.0f64; let mut si = 0.0f64;
            let w = 2.0 * std::f64::consts::PI * f as f64 / sr as f64;
            for (i, &x) in re.iter().enumerate() { sr_ += x as f64 * (w * i as f64).cos(); si -= x as f64 * (w * i as f64).sin(); }
            (sr_ * sr_ + si * si).sqrt() as f32
        };
        let mut peaks = Vec::new(); let mut floor = Vec::new();
        for k in 1..18 {
            peaks.push(dft(f0 * k as f32));
            floor.push(dft(f0 * (k as f32 + 0.5)));
        }
        if peaks.iter().chain(floor.iter()).any(|x| !x.is_finite()) { return f32::NAN; }
        peaks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        floor.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pm = peaks[peaks.len() / 2];
        let fm = floor[floor.len() / 2].max(1e-9);
        pm / fm
    }

    /// De-risk: SWEEP bow force (relative to the numerical impedance) to find the
    /// clean-Helmholtz window. The bug was bow force >> too small to ever STICK.
    ///   cargo test --release --lib archet::modal::tests::derisk -- --ignored --nocapture
    #[test]
    #[ignore]
    fn derisk() {
        // bow force = 1.0 * C01 * vb (clean Helmholtz); SWEEP B2 to find the source
        // brightness. Target RAW (pre-body) centroid ~ desired radiated: cello ~1.7k,
        // bass ~0.8k (the body then imposes formants without a big tilt).
        // LOW B2 (bright mid band) + sweep ARCHET_B3 (high-mode damping) to kill the
        // chaotic high-mode ringing without darkening. beta moderate.
        let cases = [("cello_C3", 130.8, 1.5, 0.05, 8.0e-4, 0.07, 0.20),
                     ("bass_A1", 55.0, 1.0, 0.06, 1.2e-3, 0.07, 0.18)];
        for (lbl, f0, b1, b2, stiff, beta, vb) in cases {
            let mut probe = ModalString::new(48000.0);
            probe.set_voice(f0, b1, b2, stiff, beta);
            let z = probe.impedance();
            let fb = z * vb;
            let (out, sr) = render(f0, b1, b2, stiff, beta, vb, fb, 1.6);
            let h = harmonicity(&out, sr, f0);
            let c = centroid(&out, sr, f0);
            println!("--- {} f0={:.1} beta={:.3}  harmonicity={:6.1}  raw centroid={:5.0} Hz ---",
                     lbl, f0, probe.beta, h, c);
            write_wav(&format!("/tmp/modal_{}.wav", lbl), &out, sr);
        }
    }
}
