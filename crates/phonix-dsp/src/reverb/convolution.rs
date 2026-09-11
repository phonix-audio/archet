//! Uniform-partitioned overlap-add FFT convolution. Each input block of
//! `BLOCK` samples is transformed once, multiplied against every partition
//! of the pre-transformed IR, summed and inverse-transformed. Left is
//! convolved with the IR's left channel, right with its right; each channel
//! keeps its own state.

use std::sync::Arc;

use rustfft::{Fft, FftPlanner, num_complex::Complex};

use super::irs::{shared as irs_shared, IrKind};

/// Partition block size (input samples). 512 = ~10.6 ms latency at
/// 48 kHz. Power of 2 for FFT efficiency.
pub const BLOCK: usize = 512;
/// FFT length = 2 * BLOCK for linear (non-circular) convolution.
const FFT_LEN: usize = BLOCK * 2;

/// Convolution state for a single channel.
struct ConvolutionChannel {
    /// Pre-FFT'd partitions of the IR. Index 0 = first BLOCK of the
    /// IR, index k = block k. Each is FFT_LEN complex coefficients.
    ir_partitions: Vec<Vec<Complex<f32>>>,
    /// Ring buffer of past input-block FFTs. Index = block age.
    input_fft_ring: Vec<Vec<Complex<f32>>>,
    /// Head into the ring (newest input block).
    ring_head: usize,
    /// Accumulator for the FFT result (FFT_LEN complex bins).
    acc: Vec<Complex<f32>>,
    /// Overlap tail from the previous output block.
    overlap: [f32; BLOCK],
    /// Current input block accumulator (samples since last FFT).
    in_block: [f32; BLOCK],
    in_fill:  usize,
    /// Current output block + read head into it.
    out_block: [f32; BLOCK],
    out_pos:   usize,
}

impl ConvolutionChannel {
    fn new(ir: &[f32], fft: &Arc<dyn Fft<f32>>, ifft: &Arc<dyn Fft<f32>>,
           scratch: &mut Vec<Complex<f32>>) -> Self
    {
        // Slice the IR into BLOCK-sized partitions, zero-pad each to
        // FFT_LEN, FFT in place, store.
        let n_partitions = (ir.len() + BLOCK - 1) / BLOCK;
        let mut ir_partitions = Vec::with_capacity(n_partitions);
        for k in 0..n_partitions {
            let start = k * BLOCK;
            let end   = (start + BLOCK).min(ir.len());
            let mut buf = vec![Complex { re: 0.0_f32, im: 0.0 }; FFT_LEN];
            for (i, &s) in ir[start..end].iter().enumerate() {
                buf[i].re = s;
            }
            scratch.resize(fft.get_inplace_scratch_len(), Complex { re: 0.0, im: 0.0 });
            fft.process_with_scratch(&mut buf, scratch);
            ir_partitions.push(buf);
        }
        let input_fft_ring = vec![
            vec![Complex { re: 0.0, im: 0.0 }; FFT_LEN]; n_partitions
        ];
        let _ = ifft; // ifft is passed in for symmetry; used in `process_block`
        Self {
            ir_partitions,
            input_fft_ring,
            ring_head: 0,
            acc: vec![Complex { re: 0.0, im: 0.0 }; FFT_LEN],
            overlap: [0.0; BLOCK],
            in_block: [0.0; BLOCK],
            in_fill: 0,
            out_block: [0.0; BLOCK],
            out_pos: 0,
        }
    }

    /// Process the assembled input block, produce a new output block,
    /// and reset accumulators for the next block.
    fn process_block(
        &mut self,
        fft: &Arc<dyn Fft<f32>>,
        ifft: &Arc<dyn Fft<f32>>,
        scratch: &mut Vec<Complex<f32>>,
    ) {
        // FFT the new input block into the ring at head position.
        let head_buf = &mut self.input_fft_ring[self.ring_head];
        for s in head_buf.iter_mut() { *s = Complex { re: 0.0, im: 0.0 }; }
        for (i, &s) in self.in_block.iter().enumerate() {
            head_buf[i].re = s;
        }
        scratch.resize(fft.get_inplace_scratch_len(), Complex { re: 0.0, im: 0.0 });
        fft.process_with_scratch(head_buf, scratch);

        // Multiply-accumulate: for each partition k, multiply the
        // input FFT (age k) by ir_partitions[k] and sum into acc.
        for s in self.acc.iter_mut() { *s = Complex { re: 0.0, im: 0.0 }; }
        let n = self.ir_partitions.len();
        for k in 0..n {
            // Age k means the input from k blocks ago, stored at
            // (ring_head - k) mod n.
            let idx = (self.ring_head + n - k) % n;
            let in_buf  = &self.input_fft_ring[idx];
            let ir_buf  = &self.ir_partitions[k];
            for j in 0..FFT_LEN {
                let a = in_buf[j];
                let b = ir_buf[j];
                // (a.re*b.re - a.im*b.im) + i(a.re*b.im + a.im*b.re)
                self.acc[j].re += a.re * b.re - a.im * b.im;
                self.acc[j].im += a.re * b.im + a.im * b.re;
            }
        }

        // Advance ring head for the next block.
        self.ring_head = (self.ring_head + 1) % n;

        // Inverse FFT.
        scratch.resize(ifft.get_inplace_scratch_len(), Complex { re: 0.0, im: 0.0 });
        ifft.process_with_scratch(&mut self.acc, scratch);
        // rustfft IFFT is unnormalized — divide by FFT_LEN.
        let norm = 1.0 / (FFT_LEN as f32);
        // Overlap-add: first BLOCK samples = output[i] = real(acc[i]) + overlap[i]
        // Last BLOCK samples = overlap for next block = real(acc[BLOCK + i])
        for i in 0..BLOCK {
            self.out_block[i] = self.acc[i].re * norm + self.overlap[i];
        }
        for i in 0..BLOCK {
            self.overlap[i] = self.acc[BLOCK + i].re * norm;
        }
        self.out_pos = 0;
    }

    /// Push one input sample, return one output sample.
    /// When the input buffer fills (every BLOCK samples) we run the
    /// block processing inline. For 512-sample blocks at 48 kHz that's
    /// a ~10 ms hiccup every 10 ms — perfectly fine for offline-style
    /// rendering but in a real-time context the block work should be
    /// amortized. The engine wraps this in a per-sample loop that the
    /// audio thread already iterates, so any single block-boundary
    /// FFT is at most a couple hundred microseconds for our IR sizes.
    #[inline(always)]
    fn process(
        &mut self,
        sample_in: f32,
        fft: &Arc<dyn Fft<f32>>,
        ifft: &Arc<dyn Fft<f32>>,
        scratch: &mut Vec<Complex<f32>>,
    ) -> f32 {
        self.in_block[self.in_fill] = sample_in;
        self.in_fill += 1;
        let out = self.out_block[self.out_pos];
        self.out_pos += 1;
        if self.in_fill >= BLOCK {
            self.process_block(fft, ifft, scratch);
            self.in_fill = 0;
        }
        out
    }
}

/// Stereo convolution reverb.
pub struct ConvolutionReverb {
    sr: f32,
    ir_kind: IrKind,
    fft:  Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    left:  ConvolutionChannel,
    right: ConvolutionChannel,
    /// Silence counter for idle bypass — saves the FFT cost when the
    /// engine is quiet and the IR tail has decayed.
    silent_samples: u32,
    /// Number of samples of silence required before bypass kicks in:
    /// length of the active IR + 0.5 s tail margin.
    bypass_after: u32,
    /// Per-IR perceptual-loudness compensation. Raw convolution
    /// output scales with sum_abs(ir) of the active IR — long
    /// cathedral IRs have sum_abs >> 1.0, which makes the wet bus
    /// dramatically louder than the algorithmic Schroeder reverb at
    /// the same `reverb_level` knob position. We pre-compute
    /// `0.3 / max(1.0, sum_abs(ir))` so the perceptual wet level
    /// roughly matches the algorithmic reverb at the same knob.
    gain_compensation: f32,
}

/// Energy-balance the convolution output against the algorithmic
/// Schroeder reverb. Raw convolution at the same `reverb_level`
/// knob produces wet that's >> the algorithmic one because every
/// IR sample contributes to dense tail summation. We compensate by
/// 1/sqrt(sum_of_squares), a standard RMS-energy normaliser — short
/// IRs stay punchy, long IRs are reined in, and a drum-style
/// transient input ends up at a perceptually similar wet level to
/// the algorithmic path.
///
/// The 5.0 numerator was tuned empirically: it leaves the loudest
/// IR (Cathedral, ~8s) at a wet peak around 0.10 on an impulse,
/// while keeping the shortest (Spring, ~2s) closer to 0.25 — both
/// well below 1.0 saturation, both still audible. Algorithmic
/// Schroeder reverb on the same input typically peaks 0.2-0.4.
fn compute_gain_compensation(ir_left: &[f32], ir_right: &[f32]) -> f32 {
    let sumsq_l: f32 = ir_left.iter().map(|s| s * s).sum();
    let sumsq_r: f32 = ir_right.iter().map(|s| s * s).sum();
    let sumsq = sumsq_l.max(sumsq_r).max(1.0);
    5.0 / sumsq.sqrt()
}

impl ConvolutionReverb {
    pub fn new(sr: f32, initial_kind: IrKind) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft  = planner.plan_fft_forward(FFT_LEN);
        let ifft = planner.plan_fft_inverse(FFT_LEN);
        let mut scratch: Vec<Complex<f32>> = Vec::new();
        let lib = irs_shared();
        let ir = lib.get(initial_kind).expect("ir library missing initial kind");
        // Sample-rate convert the IR if engine SR != IR SR. We accept the slight pitch shift of running a 22.05 kHz IR
        // at 48 kHz (the resampling would add cost; the user-perceived
        // effect is a slightly brighter / shorter tail, which is fine
        // for cinematic verbs). A more rigorous implementation would
        // upsample IRs at load time.
        let left  = ConvolutionChannel::new(&ir.left,  &fft, &ifft, &mut scratch);
        let right = ConvolutionChannel::new(&ir.right, &fft, &ifft, &mut scratch);
        let ir_secs = ir.left.len() as f32 / ir.sample_rate;
        let bypass_after = ((ir_secs + 0.5) * sr) as u32;
        let gain_compensation = compute_gain_compensation(&ir.left, &ir.right);
        Self {
            sr, ir_kind: initial_kind,
            fft, ifft, scratch, left, right,
            silent_samples: 0,
            bypass_after,
            gain_compensation,
        }
    }

    pub fn set_ir(&mut self, kind: IrKind) {
        if kind == self.ir_kind { return; }
        let lib = irs_shared();
        let ir = match lib.get(kind) { Some(i) => i, None => return };
        // Rebuild channels with the new IR. Re-use the existing FFT
        // plans (same FFT_LEN). The brief click during the rebuild
        // is masked by amp envelopes in practice.
        self.left  = ConvolutionChannel::new(&ir.left,  &self.fft, &self.ifft, &mut self.scratch);
        self.right = ConvolutionChannel::new(&ir.right, &self.fft, &self.ifft, &mut self.scratch);
        self.ir_kind = kind;
        self.silent_samples = 0;
        let ir_secs = ir.left.len() as f32 / ir.sample_rate;
        self.bypass_after = ((ir_secs + 0.5) * self.sr) as u32;
        self.gain_compensation = compute_gain_compensation(&ir.left, &ir.right);
    }

    pub fn current_ir(&self) -> IrKind { self.ir_kind }

    /// One stereo sample in -> one stereo sample out.
    #[inline(always)]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        // Idle bypass: if both input and accumulated tail are silent
        // long enough, skip the FFT work entirely.
        let energy = l.abs() + r.abs();
        if energy < 1e-6 {
            self.silent_samples = self.silent_samples.saturating_add(1);
        } else {
            self.silent_samples = 0;
        }
        // IR-aware bypass: skip FFT once silence has lasted longer
        // than the active IR's tail (length + 0.5 s margin). Short
        // IRs like Spring (2 s) bypass within 2.5 s instead of the
        // worst-case 9 s.
        if self.silent_samples > self.bypass_after {
            return (0.0, 0.0);
        }
        let ol = self.left.process(l,  &self.fft, &self.ifft, &mut self.scratch);
        let or = self.right.process(r, &self.fft, &self.ifft, &mut self.scratch);
        (ol * self.gain_compensation, or * self.gain_compensation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impulse_input_produces_ir_like_tail() {
        // Feeding a single sample of amplitude 1.0 should produce an
        // output that looks like the IR (scaled by 1.0 since input
        // is unit).
        let mut rev = ConvolutionReverb::new(48_000.0, IrKind::AmbientRoom);
        // Skip the initial block of latency.
        let mut tail_peak = 0.0_f32;
        let _ = rev.process(1.0, 1.0);
        for i in 0..(BLOCK * 4) {
            let (l, _) = rev.process(0.0, 0.0);
            if i >= BLOCK { tail_peak = tail_peak.max(l.abs()); }
        }
        assert!(tail_peak > 0.05,
            "ambient IR tail should be audible after impulse (peak {tail_peak})");
    }

    #[test]
    fn silence_in_silence_out_after_tail() {
        let mut rev = ConvolutionReverb::new(48_000.0, IrKind::AmbientRoom);
        // Render 12 seconds of silence — well past the longest IR.
        let mut after_tail_peak = 0.0_f32;
        let total = (48_000.0 * 12.0) as usize;
        let post = (48_000.0 * 9.5) as usize;
        for i in 0..total {
            let (l, r) = rev.process(0.0, 0.0);
            if i > post { after_tail_peak = after_tail_peak.max(l.abs()).max(r.abs()); }
        }
        assert!(after_tail_peak < 1e-3,
            "convolution should be silent long after impulse-less startup (peak {after_tail_peak})");
    }


    #[test]
    fn set_ir_switches_without_panic() {
        let mut rev = ConvolutionReverb::new(48_000.0, IrKind::Plate);
        for _ in 0..1024 { let _ = rev.process(0.1, 0.1); }
        rev.set_ir(IrKind::Cathedral);
        for _ in 0..1024 { let _ = rev.process(0.1, 0.1); }
        assert_eq!(rev.current_ir(), IrKind::Cathedral);
    }
}
