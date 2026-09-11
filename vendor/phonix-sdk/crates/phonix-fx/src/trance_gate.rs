//! Tempo-synced stereo step VCA (trance gate).
//!
//! Timing (step length, smoothing) is pushed in via `set_timing` because it
//! depends on the host tempo; the pattern itself is passed per sample.

/// Beats per gate step for a SyncedLfo-style division index.
fn div_to_beats(div: u8) -> f32 {
    match div {
        0 => 4.0,        // 1/1
        1 => 2.0,        // 1/2
        2 => 1.0,        // 1/4
        3 => 0.5,        // 1/8
        4 => 0.25,       // 1/16
        5 => 0.125,      // 1/32
        6 => 1.0 / 3.0,  // 1/8T
        _ => 1.0 / 6.0,  // 1/16T
    }
}

/// The pattern a gate plays. Plain data so both an engine patch and an effect
/// slot can produce one without either depending on the other.
#[derive(Clone, Debug)]
pub struct GateParams {
    pub steps_l: [bool; 16],
    pub steps_r: [bool; 16],
    /// Active pattern length, 1..=16.
    pub length: u8,
    /// Pushes odd steps later, 0..1.
    pub swing: f32,
    /// Dry/wet, 0 = no gating, 1 = full gate.
    pub mix: f32,
    /// Right channel mirrors the left pattern.
    pub link_stereo: bool,
}

/// Curated gate patterns.
///
/// A step gate in an insert slot wants a vocabulary, not a blank 16-step grid:
/// the grid does not fit a compact rack panel, and encoding 32 on/off flags as
/// individual effect parameters would consume the slot's whole parameter budget
/// AND break on save (parameters are stored sparsely, so an OFF step is dropped
/// and comes back ON). Draw your own pattern in an engine that has room for a
/// real editor; here you pick one.
pub const PATTERN_NAMES: [&str; 8] = [
    "8ths", "16ths", "Offbeat", "Gallop", "3 vs 4", "Trance", "Stutter", "Wide",
];

/// `(left, right, length)` as 16-bit masks, bit `i` = step `i` open.
pub const PATTERNS: [(u16, u16, u8); 8] = [
    (0b1010101010101010, 0b1010101010101010, 16), // 8ths
    (0b1111111111111111, 0b1111111111111111, 16), // 16ths (open, swing/smooth shape it)
    (0b0101010101010101, 0b0101010101010101, 16), // Offbeat
    (0b1011101110111011, 0b1011101110111011, 16), // Gallop
    (0b1001001001001001, 0b1001001001001001, 12), // 3 vs 4
    (0b1010100010101000, 0b1010100010101000, 16), // Trance
    (0b1100110011001100, 0b1100110011001100, 16), // Stutter
    (0b1010101010101010, 0b0101010101010101, 16), // Wide: L/R interleaved
];

/// Expand a 16-bit mask into per-step flags. Bit 15 is step 0, so a mask reads
/// left-to-right the way the grid draws it.
pub fn mask_to_steps(mask: u16) -> [bool; 16] {
    let mut out = [false; 16];
    for (i, o) in out.iter_mut().enumerate() { *o = mask & (1 << (15 - i)) != 0; }
    out
}

/// Flip one step of a mask.
pub fn mask_toggle(mask: u16, step: usize) -> u16 {
    mask ^ (1 << (15 - step.min(15)))
}

impl GateParams {
    /// Build from raw step masks, which is how the effect stores a pattern: two
    /// packed integers rather than 32 separate parameters. That keeps the slot's
    /// parameter budget intact and, because a mask is one value, a hand-drawn
    /// pattern survives the sparse save that would drop 32 individual OFF flags.
    pub fn from_masks(left: u16, right: u16, link_stereo: bool) -> Self {
        Self {
            steps_l: mask_to_steps(left),
            steps_r: mask_to_steps(if link_stereo { left } else { right }),
            length: 16,
            ..Default::default()
        }
    }

    /// Build the parameters for one of the curated patterns. Out-of-range falls
    /// back to the first rather than panicking: the index arrives as a float
    /// through the effect parameter surface.
    pub fn from_pattern(idx: usize, link_stereo: bool) -> Self {
        let (l, r, len) = PATTERNS[idx.min(PATTERNS.len() - 1)];
        let bits = |m: u16| {
            let mut out = [false; 16];
            for (i, o) in out.iter_mut().enumerate() { *o = m & (1 << (15 - i)) != 0; }
            out
        };
        Self {
            steps_l: bits(l),
            steps_r: if link_stereo { bits(l) } else { bits(r) },
            length: len,
            ..Default::default()
        }
    }
}

impl Default for GateParams {
    fn default() -> Self {
        // Straight 8ths on both channels: an audible, unsurprising starting
        // pattern rather than silence or a fully-open gate that looks broken.
        let mut steps = [false; 16];
        for (i, s) in steps.iter_mut().enumerate() { *s = i % 2 == 0; }
        Self {
            steps_l: steps,
            steps_r: steps,
            length: 16,
            swing: 0.0,
            mix: 1.0,
            link_stereo: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TranceGate {
    sample_rate: f32,
    samples_per_step: usize,
    sample_count: usize,
    step: usize,
    gain_l: f32,
    gain_r: f32,
    smooth_coeff: f32,
}

impl TranceGate {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            samples_per_step: (sample_rate * 0.125) as usize,
            sample_count: 0,
            step: 0,
            gain_l: 1.0,
            gain_r: 1.0,
            smooth_coeff: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) { self.sample_rate = sr; }

    pub fn set_timing(&mut self, bpm: f32, div: u8, smooth_ms: f32) {
        let beats = div_to_beats(div);
        let secs = beats * 60.0 / bpm.max(1.0);
        self.samples_per_step = ((secs * self.sample_rate) as usize).max(1);
        // One-pole smoothing coefficient from a time constant.
        let t = (smooth_ms.max(0.1) * 0.001 * self.sample_rate).max(1.0);
        self.smooth_coeff = (-1.0 / t).exp();
    }

    pub fn reset(&mut self) {
        self.sample_count = 0;
        self.step = 0;
        self.gain_l = 1.0;
        self.gain_r = 1.0;
    }

    /// Advance + apply one stereo sample.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32, cfg: &GateParams) -> (f32, f32) {
        let len = (cfg.length.clamp(1, 16)) as usize;

        // Swing pushes odd steps later.
        let swing_offset = if self.step % 2 == 1 {
            (self.samples_per_step as f32 * cfg.swing * 0.5) as usize
        } else {
            0
        };
        self.sample_count += 1;
        if self.sample_count >= self.samples_per_step + swing_offset {
            self.sample_count = 0;
            self.step = (self.step + 1) % len;
        }

        let on_l = cfg.steps_l[self.step.min(15)];
        let on_r = if cfg.link_stereo { on_l } else { cfg.steps_r[self.step.min(15)] };
        let tgt_l = if on_l { 1.0 } else { 0.0 };
        let tgt_r = if on_r { 1.0 } else { 0.0 };

        self.gain_l = tgt_l + (self.gain_l - tgt_l) * self.smooth_coeff;
        self.gain_r = tgt_r + (self.gain_r - tgt_r) * self.smooth_coeff;

        let mix = cfg.mix.clamp(0.0, 1.0);
        let g_l = self.gain_l * mix + (1.0 - mix);
        let g_r = self.gain_r * mix + (1.0 - mix);
        (l * g_l, r * g_r)
    }

    /// Step index, for a GUI playhead.
    pub fn current_step(&self) -> usize { self.step }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fully_open_gate_is_transparent() {
        let mut g = TranceGate::new(48_000.0);
        g.set_timing(120.0, 3, 1.0);
        let cfg = GateParams { steps_l: [true; 16], steps_r: [true; 16], ..Default::default() };
        for _ in 0..1000 {
            let (l, r) = g.process(0.5, -0.25, &cfg);
            assert!((l - 0.5).abs() < 1e-5 && (r + 0.25).abs() < 1e-5);
        }
    }

    #[test]
    fn mix_at_zero_bypasses_the_pattern() {
        let mut g = TranceGate::new(48_000.0);
        g.set_timing(120.0, 4, 1.0);
        let cfg = GateParams { steps_l: [false; 16], steps_r: [false; 16], mix: 0.0, ..Default::default() };
        for _ in 0..1000 {
            let (l, _) = g.process(0.5, 0.5, &cfg);
            assert!((l - 0.5).abs() < 1e-5, "mix 0 must pass audio through untouched");
        }
    }

    /// A hand-drawn pattern must survive the sparse parameter save, which drops
    /// zero-valued entries. That is the whole reason a pattern is stored as two
    /// packed masks rather than 32 on/off parameters: with flags, every OFF step
    /// would be dropped and come back ON. Only an entirely empty row is lost,
    /// and an all-closed gate is silence nobody saves on purpose.
    #[test]
    fn a_drawn_pattern_survives_a_sparse_save() {
        // Start from 8ths, then turn OFF step 0 and turn ON step 1 — exactly the
        // edits that would vanish if steps were individual parameters.
        let (mut mask, _, _) = PATTERNS[0];
        mask = mask_toggle(mask, 0);
        mask = mask_toggle(mask, 1);
        assert_ne!(mask, 0, "a mask with any step open is non-zero, so it is saved");

        // The sparse save keeps non-zero values only.
        let saved: Vec<(u8, f32)> = [(0u8, mask as f32)].into_iter()
            .filter(|&(_, v)| v != 0.0)
            .collect();
        assert_eq!(saved.len(), 1, "the mask must survive the sparse filter");

        let steps = mask_to_steps(saved[0].1 as u16);
        assert!(!steps[0], "step 0 was turned off and must stay off");
        assert!(steps[1], "step 1 was turned on and must stay on");
    }

    #[test]
    fn a_closed_step_actually_attenuates() {
        let mut g = TranceGate::new(48_000.0);
        g.set_timing(120.0, 4, 0.5);
        let cfg = GateParams { steps_l: [false; 16], steps_r: [false; 16], mix: 1.0, ..Default::default() };
        let mut last = 1.0f32;
        for _ in 0..5000 { last = g.process(1.0, 1.0, &cfg).0; }
        assert!(last.abs() < 0.01, "a closed gate should silence the signal, got {last}");
    }
}
