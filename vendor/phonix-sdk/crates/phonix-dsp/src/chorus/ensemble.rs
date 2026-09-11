//! BBD ensemble chorus: three delay lines modulated by three free-running
//! LFOs at unrelated rates, summed. The control path is evaluated once every
//! 32 samples.

use std::f64::consts::PI;

// ── Ensemble Chorus (BBD simulation) ────────────────────────

const ENSEMBLE_BUFFER_MS: f32 = 20.0;
const ENSEMBLE_LFO_RATES: [f64; 3] = [0.513, 0.73, 1.07];
const ENSEMBLE_BASE_DELAYS: [f32; 3] = [4.3, 5.1, 6.7];
const ENSEMBLE_MOD_DEPTHS: [f32; 3] = [1.2, 1.5, 2.0];

#[derive(Debug, Clone)]
pub struct EnsembleChorus {
    buffer: Vec<f32>,
    write_pos: usize,
    lfo_phases: [f64; 3],
    sample_rate: f32,
    /// Sub-Hz LFO sins, refreshed every 32 samples so the per-sample
    /// path skips three `f64::sin` evaluations. 32 samples at 48 kHz
    /// = ~1.5 kHz refresh rate, well above the audible flutter
    /// threshold of any sub-2-Hz chorus LFO.
    cached_lfos: [f32; 3],
    ctrl_counter: u32,
}

impl EnsembleChorus {
    pub fn new(sample_rate: f32) -> Self {
        let buf_size = (ENSEMBLE_BUFFER_MS * sample_rate / 1000.0) as usize + 2;
        let lfo_phases = [0.0_f64, 2.094, 4.189];
        let cached_lfos = [
            lfo_phases[0].sin() as f32,
            lfo_phases[1].sin() as f32,
            lfo_phases[2].sin() as f32,
        ];
        Self {
            buffer: vec![0.0; buf_size],
            write_pos: 0,
            lfo_phases,
            sample_rate,
            cached_lfos,
            ctrl_counter: 0,
        }
    }

    #[inline]
    pub fn process(&mut self, input: f32, depth: f32) -> (f32, f32) {
        let buf_size = self.buffer.len();
        self.buffer[self.write_pos] = input;
        // Modulo by `buf_size` (non-power-of-2 ring buffer) was a
        // hardware integer divide per audio sample. Bounded reset
        // is two ops with branch prediction (almost always not
        // taken).
        self.write_pos += 1;
        if self.write_pos >= buf_size { self.write_pos = 0; }

        let refresh = self.ctrl_counter == 0;
        let mut taps = [0.0f32; 3];
        for i in 0..3 {
            if refresh {
                self.cached_lfos[i] = self.lfo_phases[i].sin() as f32;
            }
            let lfo = self.cached_lfos[i];
            let delay_ms = ENSEMBLE_BASE_DELAYS[i] + lfo * ENSEMBLE_MOD_DEPTHS[i] * depth;
            let delay_samples = delay_ms * self.sample_rate / 1000.0;

            let read_pos = self.write_pos as f32 - delay_samples - 1.0;
            let read_pos = if read_pos < 0.0 {
                read_pos + buf_size as f32
            } else {
                read_pos
            };
            let idx = read_pos as usize % buf_size;
            let frac = read_pos.fract();
            let next = (idx + 1) % buf_size;
            taps[i] = self.buffer[idx] * (1.0 - frac) + self.buffer[next] * frac;

            self.lfo_phases[i] +=
                2.0 * PI * ENSEMBLE_LFO_RATES[i] / self.sample_rate as f64;
            if self.lfo_phases[i] > 2.0 * PI {
                self.lfo_phases[i] -= 2.0 * PI;
            }
        }
        self.ctrl_counter = (self.ctrl_counter + 1) & 31;

        let left = input * 0.6 + taps[0] * 0.35 + taps[1] * 0.15 + taps[2] * 0.25;
        let right = input * 0.6 + taps[0] * 0.15 + taps[1] * 0.35 - taps[2] * 0.25;
        (left, right)
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }
}

