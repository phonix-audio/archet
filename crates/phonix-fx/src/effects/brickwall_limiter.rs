//! A lookahead peak limiter with a Lagrange-3 inter-sample peak estimate:
//! 5 ms of lookahead, a gain ramp that lands before the peak leaves the
//! window, a one-pole release, a hard clamp at the ceiling.

use crate::effect::{Effect, Ports, Value};
use crate::spec::{Category, Curve, EffectSpec, Needs, ParamFlags, ParamKind, ParamSpec, ReadoutKind, ReadoutSpec, Unit};

pub struct BrickwallLimiter {
    sample_rate: f32,
    lookahead_samples: usize,
    /// Stereo delay line for the lookahead. The output reads from
    /// `(write_pos + 1) % lookahead_samples`, i.e. the OLDEST sample
    /// in the ring — that's `lookahead_samples - 1` samples behind
    /// the just-written input.
    delay_l: Vec<f32>,
    delay_r: Vec<f32>,
    write_pos: usize,
    /// 4-sample history used by the Lagrange-3 cubic interpolation
    /// for inter-sample peak estimation. Indices follow the
    /// convention h[0]=x[-3], h[3]=x[0] (most recent).
    hist_l: [f32; 4],
    hist_r: [f32; 4],
    /// Sliding-window peak tracking. Stores the peak associated with
    /// the sample currently driving `current_peak`, plus the count of
    /// samples until that peak ages out of the lookahead window. When
    /// it expires we rescan the delay line to find the new max.
    /// O(1) amortised — rescans are rare.
    current_peak: f32,
    peak_age:     usize,
    /// Current applied gain (linear, 0..1).
    gain:          f32,
    /// Gain we're ramping toward (linear). Reset to 1.0 when no peak
    /// demands reduction.
    target_gain:   f32,
    /// Per-sample gain decrement when ramping down — computed each
    /// time we discover a louder peak so the ramp completes within
    /// the lookahead window.
    gain_ramp:     f32,
    /// One-pole release coefficient. 1.0 = no release (hold), 0 =
    /// instant. Derived from the `release_ms` knob.
    release_coef:  f32,
    /// Output ceiling in linear amplitude (default ~0.965 = -0.3 dB).
    ceiling:       f32,
    /// Knob mirrors for serde / GUI.
    ceiling_db:    f32,
    release_ms:    f32,
}

impl BrickwallLimiter {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(1.0);
        // 5 ms lookahead — long enough that even a hard-attack
        // transient gets a smooth gain ramp without sounding
        // pre-clipped, short enough that latency is negligible
        // (5 ms is below the audibility threshold for groove
        // displacement on a master bus).
        let lookahead_samples = ((sr * 0.005) as usize).max(16);
        let mut s = Self {
            sample_rate: sr,
            lookahead_samples,
            delay_l: vec![0.0; lookahead_samples],
            delay_r: vec![0.0; lookahead_samples],
            write_pos: 0,
            hist_l: [0.0; 4],
            hist_r: [0.0; 4],
            current_peak: 0.0,
            peak_age:     0,
            gain:          1.0,
            target_gain:   1.0,
            gain_ramp:     0.0,
            release_coef:  0.0,
            ceiling:       0.965,
            ceiling_db:   -0.3,
            release_ms:   100.0,
        };
        s.set_release_ms(100.0);
        s
    }

    pub fn set_ceiling_db(&mut self, db: f32) {
        self.ceiling_db = db.clamp(-12.0, 0.0);
        self.ceiling = 10.0_f32.powf(self.ceiling_db / 20.0);
    }
    pub fn set_release_ms(&mut self, ms: f32) {
        self.release_ms = ms.clamp(5.0, 1000.0);
        // 1-pole release coef: gain_n+1 = gain_n * coef + 1 * (1-coef).
        // Reaches -3 dB toward unity in ~release_ms.
        let tau = (self.release_ms * 0.001 * self.sample_rate).max(1.0);
        self.release_coef = (-1.0_f32 / tau).exp();
    }

    /// Lagrange-3 cubic interpolation peak estimate over the 4 most
    /// recent samples. Returns the maximum |y(t)| at t in {0, 0.25,
    /// 0.5, 0.75, 1.0} — cheap 4x oversampled true-peak proxy
    /// without running a polyphase upsampler.
    #[inline]
    fn intersample_peak(h: &[f32; 4]) -> f32 {
        let x_m1 = h[0];
        let x_0  = h[1];
        let x_1  = h[2];
        let x_2  = h[3];
        let mut peak = x_0.abs().max(x_1.abs());
        // Evaluate the Lagrange-3 interpolant between x[0] and x[1]
        // at t = 0.25, 0.5, 0.75. Coefficients precomputed for each
        // grid position.
        let evaluate = |t: f32| -> f32 {
            let t2 = t * t;
            let t3 = t2 * t;
            // Standard cubic Lagrange basis on grid {-1, 0, 1, 2}.
            let c_m1 = (-t3 + 2.0 * t2 - t) / 2.0;
            let c_0  = (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0;
            let c_1  = (-3.0 * t3 + 4.0 * t2 + t) / 2.0;
            let c_2  = (t3 - t2) / 2.0;
            c_m1 * x_m1 + c_0 * x_0 + c_1 * x_1 + c_2 * x_2
        };
        peak = peak.max(evaluate(0.25).abs());
        peak = peak.max(evaluate(0.50).abs());
        peak = peak.max(evaluate(0.75).abs());
        peak
    }

    /// Rescan the entire lookahead delay line + history for the
    /// current peak. O(lookahead) but only called when the previously
    /// tracked peak ages out — amortised cost stays low.
    fn rescan_peak(&mut self) {
        let mut peak = 0.0_f32;
        for i in 0..self.lookahead_samples {
            peak = peak.max(self.delay_l[i].abs()).max(self.delay_r[i].abs());
        }
        // Don't bother re-interpolating; the per-sample push already
        // captured each intersample peak when the sample first
        // entered the window. The discrete-sample scan is enough to
        // bound the residual after the dominant peak ages out.
        self.current_peak = peak;
        self.peak_age = self.lookahead_samples;
    }

    fn tick(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        // Idle-bypass: only safe to short-circuit when ALL of these
        // hold simultaneously — input is silent, gain has released
        // back to unity, and the lookahead has aged its previous
        // peak out (so we know the delay line is silent too). At that
        // point the per-sample Lagrange-3 ISP estimate + peak-age
        // tracking + ramp logic are all wasted work; output is zero
        // anyway. Single non-silent sample reactivates the full path.
        const SILENCE_EPS: f32 = 1.0e-7;
        let abs_sum = input_l.abs() + input_r.abs();
        if abs_sum < SILENCE_EPS
            && self.gain >= 0.99999
            && self.current_peak < SILENCE_EPS
        {
            // Still advance the ring-buffer cursor so the read
            // position semantics remain consistent if the input
            // wakes up next sample.
            self.delay_l[self.write_pos] = 0.0;
            self.delay_r[self.write_pos] = 0.0;
            self.write_pos = (self.write_pos + 1) % self.lookahead_samples;
            return (0.0, 0.0);
        }

        // Push input into the lookahead delay line.
        self.delay_l[self.write_pos] = input_l;
        self.delay_r[self.write_pos] = input_r;
        // Shift the 4-sample history (h[3] = newest).
        self.hist_l[0] = self.hist_l[1]; self.hist_l[1] = self.hist_l[2];
        self.hist_l[2] = self.hist_l[3]; self.hist_l[3] = input_l;
        self.hist_r[0] = self.hist_r[1]; self.hist_r[1] = self.hist_r[2];
        self.hist_r[2] = self.hist_r[3]; self.hist_r[3] = input_r;

        // True-peak estimate for the newest sample.
        let new_isp = Self::intersample_peak(&self.hist_l)
            .max(Self::intersample_peak(&self.hist_r));

        // Sliding-window max with amortised O(1) update:
        //   * If new peak >= current peak: replace, restart age.
        //   * Else: age the current peak by one sample; on expiry,
        //     rescan the delay line.
        if new_isp >= self.current_peak {
            self.current_peak = new_isp;
            self.peak_age = self.lookahead_samples;
        } else if self.peak_age > 0 {
            self.peak_age -= 1;
            if self.peak_age == 0 { self.rescan_peak(); }
        }

        // Compute the gain that would keep this peak below the
        // ceiling. Cap at 1.0 (never make audio louder).
        let needed_gain = if self.current_peak > 1.0e-9 {
            (self.ceiling / self.current_peak).min(1.0)
        } else { 1.0 };

        // If a louder reduction is needed than the gain we're already
        // ramping toward, restart the ramp so we hit the new target
        // by the time the offending sample exits the lookahead.
        if needed_gain < self.target_gain {
            self.target_gain = needed_gain;
            let denom = self.lookahead_samples as f32;
            self.gain_ramp = (self.gain - self.target_gain) / denom;
        }

        // Apply the ramp this sample.
        if self.gain > self.target_gain {
            self.gain -= self.gain_ramp;
            if self.gain < self.target_gain { self.gain = self.target_gain; }
        } else if self.gain < 1.0 && needed_gain >= self.target_gain {
            // Release toward 1.0 (only when no new ramp is pending).
            self.target_gain = 1.0;
            self.gain = self.gain * self.release_coef
                + 1.0 * (1.0 - self.release_coef);
            if self.gain > 0.99995 { self.gain = 1.0; }
        }

        // Read the delayed sample (oldest in the ring).
        let read_pos = (self.write_pos + 1) % self.lookahead_samples;
        let dry_l = self.delay_l[read_pos];
        let dry_r = self.delay_r[read_pos];
        self.write_pos = read_pos;

        // Apply the (lookahead-aligned) gain. Final hard clamp at the
        // ceiling guards against any residual oversight in the
        // discrete-sample rescan path.
        let out_l = (dry_l * self.gain).clamp(-self.ceiling, self.ceiling);
        let out_r = (dry_r * self.gain).clamp(-self.ceiling, self.ceiling);
        (out_l, out_r)
    }

    pub fn reset(&mut self) {
        for v in &mut self.delay_l { *v = 0.0; }
        for v in &mut self.delay_r { *v = 0.0; }
        self.write_pos = 0;
        self.hist_l = [0.0; 4];
        self.hist_r = [0.0; 4];
        self.current_peak = 0.0;
        self.peak_age = 0;
        self.gain = 1.0;
        self.target_gain = 1.0;
        self.gain_ramp = 0.0;
    }

    pub fn ceiling_db(&self) -> f32 { self.ceiling_db }
    pub fn release_ms(&self) -> f32 { self.release_ms }
    pub fn current_gain_reduction_db(&self) -> f32 {
        if self.gain >= 0.99999 { 0.0 } else { 20.0 * self.gain.max(1e-9).log10() }
    }
}

pub static PARAMS: [ParamSpec; 2] = [
    ParamSpec { id: "ceiling", name: "Ceiling", short: "Ceil", kind: ParamKind::Float { min: -12.0, max: 0.0, curve: Curve::Linear }, unit: Unit::Db, default: Value::F(-0.3), flags: ParamFlags::AUTOMATABLE },
    ParamSpec { id: "release", name: "Release", short: "Rel", kind: ParamKind::Float { min: 5.0, max: 1000.0, curve: Curve::Log }, unit: Unit::Ms, default: Value::F(100.0), flags: ParamFlags::AUTOMATABLE },
];

pub static READOUTS: [ReadoutSpec; 1] = [
    ReadoutSpec { id: "gain-reduction", name: "Gain Reduction", kind: ReadoutKind::Scalar, unit: Unit::Db },
];

pub static SPEC: EffectSpec = EffectSpec {
    kind: "brickwall-limiter",
    name: "Brickwall Limiter",
    category: Category::Dynamics,
    params: &PARAMS,
    readouts: &READOUTS,
    needs: Needs::NONE,
};

pub fn build(sample_rate: f32) -> Box<dyn Effect> {
    Box::new(BrickwallLimiter::new(sample_rate))
}

impl Effect for BrickwallLimiter {
    fn spec(&self) -> &'static EffectSpec {
        &SPEC
    }

    fn set_param(&mut self, index: usize, value: Value) {
        match index {
            0 => self.set_ceiling_db(value.as_f32()),
            1 => self.set_release_ms(value.as_f32()),
            _ => {}
        }
    }

    fn param(&self, index: usize) -> Value {
        match index {
            0 => Value::F(self.ceiling_db()),
            _ => Value::F(self.release_ms()),
        }
    }

    fn process(&mut self, ports: &mut Ports<'_>) {
        for i in 0..ports.frames() {
            let (l, r) = self.tick(ports.audio.l[i], ports.audio.r[i]);
            ports.audio.l[i] = l;
            ports.audio.r[i] = r;
        }
    }

    fn reset(&mut self) {
        BrickwallLimiter::reset(self)
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = BrickwallLimiter::new(sample_rate);
    }

    fn latency_samples(&self) -> usize {
        self.lookahead_samples.saturating_sub(1)
    }

    fn readout(&self, index: usize) -> f32 {
        match index {
            0 => self.current_gain_reduction_db(),
            _ => f32::NAN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_is_sound_and_its_size_is_frozen() {
        assert!(SPEC.problems().is_empty(), "{:?}", SPEC.problems());
        assert_eq!(SPEC.params.len(), 2);
    }

    #[test]
    fn nothing_leaves_above_the_ceiling() {
        let mut lim = BrickwallLimiter::new(48_000.0);
        lim.set_param(0, Value::F(-0.3));
        let mut l: Vec<f32> = (0..48_000).map(|i| 0.99 * (i as f32 * 0.03).sin()).collect();
        let mut r = l.clone();
        let (mut sl, mut sr) = (Vec::new(), Vec::new());
        for chunk in 0..(48_000 / 256) {
            let range = chunk * 256..(chunk + 1) * 256;
            let mut ports = Ports { audio: crate::effect::StereoMut { l: &mut l[range.clone()], r: &mut r[range] }, buses: &[], sidechain: None, modulator: None, transport: Default::default(), musical: Default::default(), sample_rate: 48_000.0 };
            lim.process(&mut ports);
            sl.extend_from_slice(ports.audio.l);
            sr.extend_from_slice(ports.audio.r);
        }
        let ceiling = 10.0_f32.powf(-0.3 / 20.0);
        let peak = sl.iter().chain(sr.iter()).fold(0.0_f32, |m, v| m.max(v.abs()));
        assert!(peak <= ceiling + 1e-6, "{peak} above {ceiling}");
        assert_eq!(lim.latency_samples(), 239);
    }
}
