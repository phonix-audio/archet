//! A fader ride at full scale, the last stage of a plugin's output.
//!
//! The gain is one until a stereo pair passes full scale, then whatever
//! brings the peak back to it, reached over a short slope that a lookahead
//! of the same length hides, so a vertical edge is held too. The peak is
//! held for two periods of the lowest note the instrument plays, so the
//! gain never moves inside a cycle, and it comes back at a walking pace
//! once the peak has passed. The peak read is the one between the
//! samples, from a cubic through the last four, so what a converter
//! reconstructs is held too. An instrument whose scale is right never
//! reaches it; its level control can.

/// The fader. One per plugin, run after the effects chain.
#[derive(Debug, Clone)]
pub struct Fader {
    /// The gain applied now.
    gain: f32,
    /// Per-sample share of the way to a lower target the gain moves.
    slope: f32,
    /// The samples in flight, oldest at `at`.
    line: [[f32; 2]; Fader::LATENCY],
    at: usize,
    /// The last four samples of each channel, for the peak between them.
    last: [[f32; 4]; 2],
    /// The highest peak seen since the hold began.
    held: f32,
    /// Samples the held peak is kept before it starts to fall.
    hold_left: usize,
    /// How many samples a new peak is held.
    hold: usize,
    /// Per-sample factor the held peak falls by once its hold has run out.
    fall: f32,
}

impl Fader {
    /// Samples the output runs behind the input: the gain's move down is
    /// six time constants long and lands before the peak leaves the line.
    pub const LATENCY: usize = 32;
    /// The pace the gain comes back at, in dB per second.
    pub const RETURN_DB_PER_S: f32 = 6.0;
    /// Periods of the lowest note the peak is held for.
    pub const HOLD_PERIODS: f32 = 2.0;

    /// A fader for `sample_rate`, holding peaks for two periods of
    /// `lowest_hz`, the lowest note the instrument plays.
    pub fn new(sample_rate: f32, lowest_hz: f32) -> Self {
        Self {
            gain: 1.0,
            slope: 1.0 - (-6.0 / Self::LATENCY as f32).exp(),
            line: [[0.0; 2]; Self::LATENCY],
            at: 0,
            last: [[0.0; 4]; 2],
            held: 0.0,
            hold_left: 0,
            hold: (Self::HOLD_PERIODS * sample_rate / lowest_hz.max(1.0)).ceil() as usize,
            fall: 10f32.powf(-Self::RETURN_DB_PER_S / 20.0 / sample_rate.max(1.0)),
        }
    }

    /// The gain applied to the pair leaving now.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    /// Silence in the line and the gain back at one.
    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.line = [[0.0; 2]; Self::LATENCY];
        self.at = 0;
        self.last = [[0.0; 4]; 2];
        self.held = 0.0;
        self.hold_left = 0;
    }

    /// A stereo pair in, the pair from `LATENCY` samples ago out, at the
    /// gain the pair in has moved to.
    #[inline]
    pub fn run(&mut self, l: f32, r: f32) -> (f32, f32) {
        let peak = self.push(0, l).max(self.push(1, r));
        let g = self.advance(peak);
        let out = self.line[self.at];
        self.line[self.at] = [l, r];
        self.at = (self.at + 1) % Self::LATENCY;
        (out[0] * g, out[1] * g)
    }

    /// Two planar channels through the fader, in place.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            (*a, *b) = self.run(*a, *b);
        }
    }

    /// A sample into a channel's history, and the peak of the signal
    /// between the two before it: the cubic through the last four,
    /// read at quarter points. One sample behind the input, which the
    /// line covers many times over.
    #[inline]
    fn push(&mut self, ch: usize, x: f32) -> f32 {
        let h = &mut self.last[ch];
        h.rotate_left(1);
        h[3] = x;
        let [a, b, c, d] = *h;
        let mut peak = b.abs().max(c.abs());
        for t in [0.25f32, 0.5, 0.75] {
            let (t2, t3) = (t * t, t * t * t);
            let y = (-t3 + 2.0 * t2 - t) * 0.5 * a
                + (3.0 * t3 - 5.0 * t2 + 2.0) * 0.5 * b
                + (-3.0 * t3 + 4.0 * t2 + t) * 0.5 * c
                + (t3 - t2) * 0.5 * d;
            peak = peak.max(y.abs());
        }
        peak
    }

    /// The gain after a pair with this peak has entered the line.
    fn advance(&mut self, peak: f32) -> f32 {
        if peak >= self.held {
            self.held = peak;
            self.hold_left = self.hold;
        } else if self.hold_left > 0 {
            self.hold_left -= 1;
        } else {
            self.held *= self.fall;
        }
        let target = if self.held > 1.0 { 1.0 / self.held } else { 1.0 };
        if target < self.gain {
            self.gain += (target - self.gain) * self.slope;
        } else {
            self.gain = target;
        }
        self.gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// A sine at `amp` for `n` samples, both channels alike.
    fn tone(amp: f32, hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SR).sin()).collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    /// Under full scale the fader is a delay line and nothing else.
    #[test]
    fn under_full_scale_it_does_nothing() {
        let mut f = Fader::new(SR, 41.0);
        let x = tone(0.9, 220.0, 4800);
        let (mut l, mut r) = (x.clone(), x.clone());
        f.process(&mut l, &mut r);
        assert_eq!(f.gain(), 1.0);
        for (i, v) in l.iter().enumerate().skip(Fader::LATENCY) {
            assert!((v - x[i - Fader::LATENCY]).abs() < 1e-6);
        }
    }

    /// A tone twice full scale leaves at full scale within the slope's
    /// residual, and a vertical edge at four times is held too.
    #[test]
    fn past_full_scale_it_holds() {
        let mut f = Fader::new(SR, 41.0);
        let x = tone(2.0, 110.0, 9600);
        let (mut l, mut r) = (x.clone(), x.clone());
        f.process(&mut l, &mut r);
        assert!(peak(&l[Fader::LATENCY * 2..]) <= 1.02, "{}", peak(&l));
        assert!(peak(&l[4800..]) > 0.9);
        let mut f = Fader::new(SR, 41.0);
        let mut step: Vec<f32> = vec![0.0; 200];
        step.extend(std::iter::repeat_n(4.0, 800));
        let mut r = step.clone();
        f.process(&mut step, &mut r);
        assert!(peak(&step) <= 1.02, "{}", peak(&step));
    }

    /// A high tone whose samples all stay under full scale while the
    /// wave between them passes it is held on the peak between: the
    /// cubic reads most of the overshoot at a quarter of the rate.
    #[test]
    fn it_reads_the_peak_between_samples() {
        let mut f = Fader::new(SR, 41.0);
        // At a quarter of the rate, phase-shifted, the samples sit at
        // amp/sqrt(2) while the wave reaches amp.
        let n = 4800;
        let x: Vec<f32> = (0..n)
            .map(|i| 1.3 * (2.0 * std::f32::consts::PI * (SR / 4.0) * i as f32 / SR + std::f32::consts::FRAC_PI_4).sin())
            .collect();
        assert!(peak(&x) < 0.95, "the samples themselves pass full scale: {}", peak(&x));
        let (mut l, mut r) = (x.clone(), x.clone());
        f.process(&mut l, &mut r);
        assert!(f.gain() < 0.9, "the fader did not see the peak between samples: gain {}", f.gain());
    }

    /// The held peak is kept for the hold and then falls at the return
    /// pace: after one second the gain has come back six decibels.
    #[test]
    fn it_comes_back_at_a_walking_pace() {
        let mut f = Fader::new(SR, 41.0);
        let mut g = f.advance(4.0);
        for _ in 0..f.hold {
            g = f.advance(0.0);
        }
        assert!((g - 0.25).abs() < 1e-2, "the slope did not reach its target within the hold: {g}");
        for _ in 0..(SR as usize) {
            f.advance(0.0);
        }
        let g = f.advance(0.0);
        assert!((20.0 * (g / 0.25).log10() - Fader::RETURN_DB_PER_S).abs() < 0.05, "gain {g}");
    }
}
