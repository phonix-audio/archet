//! Schroeder reverb: four combs and two allpasses per channel, damped
//! feedback, a fixed output gain that lands the tail at the loudness of the
//! convolution reverbs at the same mix.

// ── Simple Schroeder Reverb ─────────────────────────────────

#[derive(Debug, Clone)]
struct CombFilter {
    buffer: Vec<f32>,
    pos: usize,
    feedback: f32,
    damp: f32,
    damp_state: f32,
}

impl CombFilter {
    pub fn set_feedback(&mut self, fb: f32) {
        self.feedback = fb.clamp(0.0, 0.99);
    }
    pub fn set_damp(&mut self, d: f32) {
        self.damp = d.clamp(0.0, 0.95);
    }

    fn new(delay_samples: usize, feedback: f32, damp: f32) -> Self {
        Self {
            buffer: vec![0.0; delay_samples.max(1)],
            pos: 0,
            feedback,
            damp,
            damp_state: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.pos];
        self.damp_state = delayed * (1.0 - self.damp) + self.damp_state * self.damp;
        self.buffer[self.pos] = input + self.damp_state * self.feedback;
        self.pos = (self.pos + 1) % self.buffer.len();
        delayed
    }
}

#[derive(Debug, Clone)]
struct AllPassFilter {
    buffer: Vec<f32>,
    pos: usize,
    feedback: f32,
}

impl AllPassFilter {
    fn new(delay_samples: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; delay_samples.max(1)],
            pos: 0,
            feedback,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.pos];
        let output = -input + delayed;
        self.buffer[self.pos] = input + delayed * self.feedback;
        self.pos = (self.pos + 1) % self.buffer.len();
        output
    }
}

#[derive(Debug, Clone)]
pub struct SimpleReverb {
    combs_l: [CombFilter; 4],
    combs_r: [CombFilter; 4],
    allpasses_l: [AllPassFilter; 2],
    allpasses_r: [AllPassFilter; 2],
    /// Output gain to match convolution reverb's perceived loudness.
    /// Without this the algorithmic branch was ~3× quieter than
    /// convolution at the same `reverb_mix` knob position.
    output_gain: f32,
}

impl SimpleReverb {
    pub fn new(sample_rate: f32) -> Self {
        let ms = |t: f32| (t * sample_rate / 1000.0) as usize;
        let fb = 0.75;
        let damp = 0.4;

        Self {
            combs_l: [
                CombFilter::new(ms(29.7), fb, damp),
                CombFilter::new(ms(37.1), fb, damp),
                CombFilter::new(ms(41.1), fb, damp),
                CombFilter::new(ms(43.7), fb, damp),
            ],
            combs_r: [
                CombFilter::new(ms(31.3), fb, damp),
                CombFilter::new(ms(38.9), fb, damp),
                CombFilter::new(ms(42.3), fb, damp),
                CombFilter::new(ms(45.1), fb, damp),
            ],
            allpasses_l: [
                AllPassFilter::new(ms(5.0), 0.5),
                AllPassFilter::new(ms(1.7), 0.5),
            ],
            allpasses_r: [
                AllPassFilter::new(ms(5.3), 0.5),
                AllPassFilter::new(ms(1.9), 0.5),
            ],
            output_gain: 3.0,
        }
    }

    /// `size` 0..1 → comb feedback 0.5..0.9. Drives the apparent
    /// room size / tail length.
    pub fn set_size(&mut self, size: f32) {
        let fb = 0.5 + size.clamp(0.0, 1.0) * 0.4;
        for c in &mut self.combs_l { c.set_feedback(fb); }
        for c in &mut self.combs_r { c.set_feedback(fb); }
    }

    /// Per-IR high-frequency damping (0..1). Higher = darker.
    /// Lets the IR-kind dropdown sculpt timbre on the algorithmic
    /// branch instead of being inert. Cathedral=0.05, Wooden Hall
    /// =0.80, etc. — see `IrKind::algorithmic_params`.
    pub fn set_damping(&mut self, damp: f32) {
        let d = damp.clamp(0.0, 0.95);
        for c in &mut self.combs_l { c.set_damp(d); }
        for c in &mut self.combs_r { c.set_damp(d); }
    }

    #[inline]
    pub fn process(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut out_l = 0.0;
        let mut out_r = 0.0;
        for comb in &mut self.combs_l {
            out_l += comb.process(left);
        }
        for comb in &mut self.combs_r {
            out_r += comb.process(right);
        }
        out_l *= 0.25;
        out_r *= 0.25;

        for ap in &mut self.allpasses_l {
            out_l = ap.process(out_l);
        }
        for ap in &mut self.allpasses_r {
            out_r = ap.process(out_r);
        }
        (out_l * self.output_gain, out_r * self.output_gain)
    }
}

