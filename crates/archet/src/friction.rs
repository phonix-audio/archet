//! Bow-string friction at the scattering junction.
//!
//! Two models, selectable per patch:
//!
//! - `Static`: the canonical STK `BowTable` hyperbolic reflection curve
//!   `rho = min((|dv*slope| + 0.75)^-4, 1)` (Cook/Scavone after J.O. Smith).
//!   Cheap, unconditionally stable, gives a Helmholtz sawtooth. The corner is
//!   slightly rounded, so the raw spectrum rolls off above ~3 harmonics.
//!
//! - `ElastoPlastic`: a bristle friction model with a Stribeck steady-state and
//!   an elasto-plastic adhesion map (Serafin/Avanzini/Rocchesso SMAC 2003;
//!   Dupont et al. IEEE TAC 47, 2002). The history-dependent stick->slip
//!   break-away sharpens the Helmholtz corner (richer harmonics) and produces
//!   realistic pre-Helmholtz attack transients the memoryless curve cannot.
//!
//! Both return the *injected velocity* `new_vel` added to both outgoing waves at
//! the bow junction (STK convention: `dv = bow_vel - string_vel`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrictionMode {
    Static,
    ElastoPlastic,
}

#[derive(Debug, Clone)]
pub struct Friction {
    pub mode: FrictionMode,
    sr: f32,

    // --- static bow table ---
    /// Base slope; effective slope is divided by bow force (more force => wider
    /// capture region => brighter/louder, per Schelleng).
    pub slope: f32,

    // --- elasto-plastic ---
    pub sigma0: f32, // bristle stiffness (N/m, normalized)
    pub sigma1: f32, // bristle damping
    pub sigma2: f32, // viscous
    pub mu_c: f32,   // Coulomb (dynamic) friction
    pub mu_s: f32,   // static (stiction) friction
    pub vs: f32,     // Stribeck velocity (m/s)
    pub ep_gain: f32, // force -> injected-velocity scale (junction 1/2Z, tuned)
    z: f32,          // mean bristle deflection (state)
}

impl Friction {
    pub fn new(sr: f32) -> Self {
        Self {
            mode: FrictionMode::Static,
            sr,
            slope: 3.0,
            // sigma0/mu/vs: Serafin/Avanzini SMAC 2003. sigma1 = 0.5 is the
            // passivity-guaranteed value (Matusiak/Chatziioannou/Van Walstijn,
            // Frontiers 2025); the 2*sqrt(sigma0) critical-damping heuristic
            // makes the sigma1*(dz/dt) term dominate and the junction blow up.
            sigma0: 1.0e4,
            sigma1: 0.5,
            sigma2: 0.4,
            mu_c: 0.3,
            mu_s: 0.8,
            vs: 0.1,
            // f is O(mu*force) ~ O(0.3); the junction maps force -> injected
            // velocity ~ bow_vel, hence ~0.6. Tuned in the elasto de-risk.
            ep_gain: 0.6,
            z: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.z = 0.0;
    }

    /// Injected velocity at the bow junction.
    /// `bow_vel`: commanded bow speed. `string_vel`: incoming string velocity at
    /// the bow point. `force`: normalized bow force (>0).
    #[inline]
    pub fn tick(&mut self, bow_vel: f32, string_vel: f32, force: f32) -> f32 {
        // Bow lifted: no contact, no excitation -- the string rings down through
        // its own losses (fixes the continuous drone after note-off).
        if force < 1.0e-3 {
            self.z = 0.0;
            return 0.0;
        }
        match self.mode {
            FrictionMode::Static => {
                let dv = bow_vel - string_vel;
                // Higher force narrows slope -> wider stick capture.
                let slope = self.slope / force.max(0.2);
                let s = (dv * slope).abs() + 0.75;
                let coeff = (s * s * s * s).recip().min(1.0);
                dv * coeff
            }
            FrictionMode::ElastoPlastic => self.elasto(bow_vel, string_vel, force),
        }
    }

    /// Stribeck steady-state bristle deflection magnitude for a slip velocity.
    #[inline]
    fn stribeck(&self, vrel: f32, fc: f32, fs: f32) -> f32 {
        let g = fc + (fs - fc) * (-(vrel / self.vs).powi(2)).exp(); // friction magnitude, >0
        if vrel.abs() > 1e-9 { (g / self.sigma0) * vrel.signum() } else { 0.0 }
    }

    /// Elasto-plastic adhesion map alpha in [0,1]: 0 = pure stick (presliding,
    /// z still building), 1 = pure slip. Smooth sin transition in between
    /// (Dupont et al. 2002; Serafin SMAC 2003).
    #[inline]
    fn adhesion(z: f32, z_ss: f32) -> f32 {
        if z_ss.abs() < 1e-12 { return 0.0; }
        let r = z / z_ss;
        if r <= 0.5 { 0.0 }
        else if r >= 1.0 { 1.0 }
        else { 0.5 * (1.0 + ((r - 0.75) / 0.25 * std::f32::consts::FRAC_PI_2).sin()) }
    }

    /// Normalized friction force f(vrel, z) at the bow (units of injected
    /// velocity; the string wave impedance 2Z is folded into the coefficients).
    #[inline]
    fn friction_force(&self, vrel: f32, z: f32, fc: f32, fs: f32) -> f32 {
        let z_ss = self.stribeck(vrel, fc, fs);
        let alpha = Self::adhesion(z, z_ss);
        let zdot = vrel * (1.0 - if z_ss.abs() > 1e-12 { alpha * z / z_ss } else { 0.0 });
        self.sigma0 * z + self.sigma1 * zdot + self.sigma2 * vrel
    }

    /// Elasto-plastic bowed junction, solved IMPLICITLY. The bow injects a
    /// velocity `x` such that the friction force and the resulting slip velocity
    /// are mutually consistent: `x + f(x - vDelta, z) = 0`, where
    /// `vDelta = bow_vel - v_h`. Solved by damped Newton with a finite-difference
    /// derivative (robust across the stiff stick region), then the bristle state
    /// `z` is integrated with the converged slip velocity. This is the standard
    /// DWG bowed-string scattering (Smith PASP; Serafin/Avanzini SMAC 2003) and
    /// is what gives a real bow's grip + hysteretic attack, unlike the memoryless
    /// static curve.
    #[inline]
    fn elasto(&mut self, bow_vel: f32, v_h: f32, force: f32) -> f32 {
        let v_delta = bow_vel - v_h;
        let fc = self.mu_c * force;
        let fs = self.mu_s * force;
        let z = self.z;

        // Newton solve for the injected velocity x.
        let mut x = v_delta * 0.5; // warm start between free (0) and full stick (vDelta)
        let h = 1e-4f32;
        for _ in 0..6 {
            let f0 = self.friction_force(x - v_delta, z, fc, fs);
            let psi = x + f0;
            if psi.abs() < 1e-7 { break; }
            let f1 = self.friction_force((x + h) - v_delta, z, fc, fs);
            let dpsi = 1.0 + (f1 - f0) / h;
            x -= psi / dpsi.clamp(0.25, 1.0e6);
        }

        // Integrate the bristle state with the converged slip velocity.
        let vrel = x - v_delta;
        let z_ss = self.stribeck(vrel, fc, fs);
        let alpha = Self::adhesion(z, z_ss);
        let zdot = vrel * (1.0 - if z_ss.abs() > 1e-12 { alpha * z / z_ss } else { 0.0 });
        self.z += zdot / self.sr;
        // Clamp z to its physical ceiling (full-slip deflection) to bar runaway.
        let z_cap = (fs / self.sigma0) * 1.5;
        self.z = self.z.clamp(-z_cap, z_cap);

        x.clamp(-1.0, 1.0)
    }
}
