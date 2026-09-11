//! Spring reverb: four diffusion taps and a modulated delay.

// ── Spring reverb (improved: 4-tap diffusion + modulated delay) ────────────

#[derive(Debug, Clone)]
pub struct SpringReverb {
    delay1: Vec<f32>,
    delay2: Vec<f32>,
    delay3: Vec<f32>,
    delay4: Vec<f32>,
    pos1: usize,
    pos2: usize,
    pos3: usize,
    pos4: usize,
    lp1: f32,
    lp2: f32,
    mod_phase: f32,
    sr: f32,
}

impl SpringReverb {
    pub fn new(sr: f32) -> Self {
        Self {
            delay1: vec![0.0; (sr * 0.031) as usize],   // 31ms
            delay2: vec![0.0; (sr * 0.047) as usize],   // 47ms
            delay3: vec![0.0; (sr * 0.071) as usize],   // 71ms
            delay4: vec![0.0; (sr * 0.019) as usize],   // 19ms
            pos1: 0, pos2: 0, pos3: 0, pos4: 0,
            lp1: 0.0, lp2: 0.0,
            mod_phase: 0.0,
            sr,
        }
    }

    #[inline(always)]
    pub fn process(&mut self, input: f32, mix: f32, decay: f32) -> (f32, f32) {
        // Modulated read for spring-like warble
        self.mod_phase += 1.5 / self.sr;
        if self.mod_phase >= 1.0 { self.mod_phase -= 1.0; }

        let d1 = self.delay1[self.pos1];
        let d2 = self.delay2[self.pos2];
        let d3 = self.delay3[self.pos3];
        let d4 = self.delay4[self.pos4];

        let fb = decay.clamp(0.3, 0.85);
        // Cross-coupled feedback for density
        let fb1 = input + d2 * fb;
        let fb2 = input * 0.8 + d1 * fb * 0.9;

        // LP damping (springs absorb highs)
        self.lp1 += (fb1 - self.lp1) * 0.25;
        self.lp2 += (fb2 - self.lp2) * 0.2;

        self.delay1[self.pos1] = self.lp1;
        self.delay2[self.pos2] = self.lp2;
        self.delay3[self.pos3] = (d1 + d4) * fb * 0.7;
        self.delay4[self.pos4] = (d2 + d3) * fb * 0.6;

        self.pos1 = (self.pos1 + 1) % self.delay1.len();
        self.pos2 = (self.pos2 + 1) % self.delay2.len();
        self.pos3 = (self.pos3 + 1) % self.delay3.len();
        self.pos4 = (self.pos4 + 1) % self.delay4.len();

        let wet_l = d1 * 0.5 + d3 * 0.3 + d4 * 0.2;
        let wet_r = d2 * 0.5 + d4 * 0.3 + d3 * 0.2;
        let dry = input * (1.0 - mix);
        // SPRING_WET_GAIN compensates for the shorter-tail spring
        // model's lower perceived loudness vs the convolution branch
        // at the same mix knob. 1.2 (not 3.0 — spring is naturally
        // louder per echo than a long-tail Schroeder reverb). User
        // bug: "algorithmic spring barely audible" on VCS-3 with the
        // same reverb knob position as convolution.
        const SPRING_WET_GAIN: f32 = 1.2;
        (dry + wet_l * mix * SPRING_WET_GAIN,
         dry + wet_r * mix * SPRING_WET_GAIN)
    }

    pub fn reset(&mut self) {
        self.delay1.fill(0.0);
        self.delay2.fill(0.0);
        self.delay3.fill(0.0);
        self.delay4.fill(0.0);
        self.pos1 = 0; self.pos2 = 0; self.pos3 = 0; self.pos4 = 0;
        self.lp1 = 0.0; self.lp2 = 0.0;
        self.mod_phase = 0.0;
    }
}
