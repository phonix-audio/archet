//! The thermal friction law of a bow on a string: Woodhouse, Bowed string
//! simulation using a thermal friction model, Acta Acustica 89, 2003,
//! equation (1), with the rosin and contact values of its Table I.
//!
//! The rosin's shear strength is a function of the contact temperature, and
//! the temperature follows the heat the sliding generates: friction power
//! against conduction into the bow and the string, convection of cold rosin
//! into the contact, and the contact volume's own heat capacity. The
//! friction coefficient therefore lags the sliding speed, which is what
//! makes Helmholtz motion establish quickly and reliably.
//!
//! Conduction into the two bodies is a half-derivative of the temperature,
//! realised as a bank of one-pole memories whose weights sum to the t^-1/2
//! kernel, plus the steady constriction conductance of a contact of radius
//! a. The coefficient's dependence on temperature is not tabulated: it is
//! the steady-sliding curve (equation 2 of the paper) mapped through this
//! contact's own steady temperature, so a steady slide reproduces that
//! curve exactly whatever the transient kernel.

/// Rosin: density (kg/m3), specific heat (J/kg K) and the active layer's
/// thickness (m), Table I.
const RHO: f64 = 1010.0;
const CP: f64 = 250.0;
const DELTA: f64 = 1.0e-6;
/// Contact radius at one newton of normal load (m); the contact area is
/// proportional to the load.
const RADIUS_1N: f64 = 250.0e-6;
/// Thermal conductivity of the string and of the bow (W/m K), Table I.
const K_STRING: f64 = 1.4;
const K_BOW: f64 = 0.2;
/// The thermal parameter sqrt(K rho c) of the string and of the bow
/// (W s^1/2 / m2 K), Table I.
const LAMBDA_STRING: f64 = 2050.0;
const LAMBDA_BOW: f64 = 450.0;

/// The memories' time constants: from one sample to about a second, three
/// to the decade.
const MEMORIES: usize = 15;
const TAU_MIN: f64 = 1.0e-5;
const PER_DECADE: f64 = 3.0;

/// The friction coefficient of rosin in steady sliding at speed `v` (m/s):
/// the curve fit of the measured values.
pub fn steady_mu(v: f64) -> f64 {
    0.4 * (-v / 0.01).exp() + 0.45 * (-v / 0.1).exp() + 0.35
}

/// Points of the temperature to friction map, from the steady slide.
const MAP_POINTS: usize = 256;

#[derive(Clone, Debug)]
pub struct Thermal {
    dt: f64,
    /// Contact temperature above ambient (K) and its previous value.
    temp: f64,
    /// The one-pole memories of the conduction half-derivative.
    mem: [f64; MEMORIES],
    decay: [f64; MEMORIES],
    gain: [f64; MEMORIES],
    weight: [f64; MEMORIES],
    /// Temperature to coefficient: the steady slide's, sampled on a speed
    /// grid, temperatures rising.
    map_t: Vec<f64>,
    map_mu: Vec<f64>,
}

impl Thermal {
    pub fn new(sr: f32) -> Self {
        let dt = 1.0 / sr as f64;
        let mut decay = [0.0; MEMORIES];
        let mut gain = [0.0; MEMORIES];
        let mut weight = [0.0; MEMORIES];
        let dln = std::f64::consts::LN_10 / PER_DECADE;
        for k in 0..MEMORIES {
            let tau = TAU_MIN * 10f64.powf(k as f64 / PER_DECADE);
            let s = 1.0 / tau;
            decay[k] = (-s * dt).exp();
            // Ramp input over the step: the memory moves by (1 - d)/(s dt)
            // per kelvin of change.
            gain[k] = (1.0 - decay[k]) / (s * dt);
            // t^-1/2 = (1/sqrt(pi)) integral of s^1/2 exp(-s t) d ln s
            weight[k] = s.sqrt() * dln / std::f64::consts::PI.sqrt();
        }
        // The steady slide at one newton: heat power over the steady
        // conductance and the convection. Speeds on a log grid make the
        // temperatures rise.
        let g_ss = Self::steady_conductance(RADIUS_1N);
        let conv = |v: f64| RADIUS_1N * DELTA * RHO * CP * v;
        let mut map_t = Vec::with_capacity(MAP_POINTS);
        let mut map_mu = Vec::with_capacity(MAP_POINTS);
        map_t.push(0.0);
        map_mu.push(steady_mu(0.0));
        for i in 0..MAP_POINTS {
            let v = 1.0e-4 * 10f64.powf(4.0 * i as f64 / (MAP_POINTS - 1) as f64);
            let mu = steady_mu(v);
            let t = mu * v / (g_ss + conv(v));
            if t > *map_t.last().unwrap() {
                map_t.push(t);
                map_mu.push(mu);
            }
        }
        Self { dt, temp: 0.0, mem: [0.0; MEMORIES], decay, gain, weight, map_t, map_mu }
    }

    /// The constriction conductance of a contact of radius `a` into the
    /// string and the bow (W/K).
    fn steady_conductance(a: f64) -> f64 {
        4.0 * a * (K_STRING + K_BOW)
    }

    /// The bow lifts: the contact cools to ambient.
    pub fn reset(&mut self) {
        self.temp = 0.0;
        self.mem = [0.0; MEMORIES];
    }

    /// The friction coefficient at the contact's temperature now.
    #[inline]
    pub fn mu(&self) -> f64 {
        let (t, m) = (&self.map_t, &self.map_mu);
        let x = self.temp;
        if x >= t[t.len() - 1] {
            return m[m.len() - 1];
        }
        // Binary search for the cell; the map is small and monotone.
        let i = t.partition_point(|&p| p <= x).clamp(1, t.len() - 1);
        let f = (x - t[i - 1]) / (t[i] - t[i - 1]);
        m[i - 1] + f * (m[i] - m[i - 1])
    }

    /// Advance the temperature one sample under `power` watts of friction
    /// at `speed` m/s of sliding and a normal load of `load` newtons.
    #[inline]
    pub fn step(&mut self, power: f64, speed: f64, load: f64) {
        let a = RADIUS_1N * load.max(1.0e-6).sqrt();
        let area = std::f64::consts::PI * a * a;
        let c_cond = area * (LAMBDA_STRING + LAMBDA_BOW) / std::f64::consts::PI.sqrt();
        let cap = RHO * area * DELTA * CP;
        let conv = a * DELTA * RHO * CP * speed;
        let g_ss = Self::steady_conductance(a);
        let mut hist = 0.0;
        let mut e = 0.0;
        for k in 0..MEMORIES {
            hist += self.weight[k] * self.decay[k] * self.mem[k];
            e += self.weight[k] * self.gain[k];
        }
        // Power balance with the half-derivative's history, implicit in the
        // new temperature: P = (g_ss + conv) T + C (H + E (T - T0))
        //                      + cap (T - T0) / dt
        let t0 = self.temp;
        let denom = g_ss + conv + c_cond * e + cap / self.dt;
        let num = power - c_cond * hist + (c_cond * e + cap / self.dt) * t0;
        let t = (num / denom).max(0.0);
        let dtemp = t - t0;
        for k in 0..MEMORIES {
            self.mem[k] = self.decay[k] * self.mem[k] + self.gain[k] * dtemp;
        }
        self.temp = t;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A steady slide settles on the steady-sliding curve.
    #[test]
    fn steady_slide_matches_the_measured_curve() {
        let mut th = Thermal::new(48_000.0);
        for &v in &[0.02f64, 0.05, 0.2] {
            th.reset();
            let load = 1.0;
            for _ in 0..(48_000 * 6) {
                let mu = th.mu();
                th.step(mu * load * v, v, load);
            }
            let want = steady_mu(v);
            assert!((th.mu() - want).abs() < 0.03, "v {v}: mu {} want {want}", th.mu());
        }
    }
}
