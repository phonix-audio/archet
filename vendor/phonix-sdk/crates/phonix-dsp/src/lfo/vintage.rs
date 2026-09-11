//! A tempo-syncable, retriggerable LFO with six shapes, distinct from
//! `lfo::Lfo` and deliberately not merged with it: the two are not
//! interchangeable, and instruments voiced on one would move on the other.
//!
//! Designed to be cheap on the audio thread: per-sample tick is one
//! float add + one branchless shape eval. No allocation, no `sin`
//! call when in sync mode (phase is normalised 0..1; the shape table
//! handles the wrap).

/// LFO shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum LfoShape {
    Sine,
    Triangle,
    Square,
    Saw,
    SampleHold,
    SmoothRandom,
}

pub struct SyncedLfo {
    sample_rate: f32,
    /// Phase in 0..1.
    phase:        f32,
    /// Per-sample phase increment when free-running (sync OFF).
    phase_inc:    f32,
    /// Per-sample phase increment when tempo-synced. Recomputed
    /// from BPM + division. Same units as `phase_inc`.
    sync_inc:     f32,
    shape:        LfoShape,
    /// Sample-and-hold cached value (regenerated when phase wraps).
    sh_value:     f32,
    /// Random walk state for `SmoothRandom`.
    sr_target:    f32,
    sr_current:   f32,
    /// Internal RNG state for S&H / SmoothRandom.
    rng:          u32,
}

impl SyncedLfo {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate, phase: 0.0,
            phase_inc: 0.0, sync_inc: 0.0,
            shape: LfoShape::Sine,
            sh_value: 0.0,
            sr_target: 0.0, sr_current: 0.0,
            rng: 0x9E37_79B9,
        }
    }

    pub fn set_rate_hz(&mut self, rate_hz: f32) {
        self.phase_inc = rate_hz.max(0.0) / self.sample_rate;
    }

    /// Tempo-sync rate from a division index. Division 0..7 maps to
    /// 1/1, 1/2, 1/4, 1/8, 1/16, 1/32, 1/8 triplet, 1/16 triplet.
    /// The LFO completes ONE cycle per division.
    pub fn set_sync(&mut self, bpm: f32, sync_div: u8) {
        let beats_per_cycle = match sync_div {
            0 => 4.0,           // 1/1 (1 cycle per bar at 4/4)
            1 => 2.0,           // 1/2
            2 => 1.0,           // 1/4
            3 => 0.5,           // 1/8
            4 => 0.25,          // 1/16
            5 => 0.125,         // 1/32
            6 => 1.0 / 3.0,     // 1/8 triplet
            _ => 1.0 / 6.0,     // 1/16 triplet
        };
        let cycles_per_sec = (bpm / 60.0) / beats_per_cycle;
        self.sync_inc = cycles_per_sec / self.sample_rate;
    }

    pub fn set_shape(&mut self, shape: LfoShape) {
        self.shape = shape;
    }

    /// Reset phase to 0 (used by per-step retrigger).
    pub fn retrigger(&mut self) {
        self.phase = 0.0;
        // Refresh S&H / random so the new cycle starts deterministically.
        self.sh_value = self.next_rand() * 2.0 - 1.0;
        self.sr_target = self.next_rand() * 2.0 - 1.0;
    }

    #[inline(always)]
    fn next_rand(&mut self) -> f32 {
        // Xorshift32.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng & 0xFFFF) as f32 / 65535.0
    }

    /// Advance one sample. Returns the LFO value in -1..+1.
    /// `use_sync` selects between free-run rate and tempo-sync rate.
    #[inline(always)]
    pub fn tick(&mut self, use_sync: bool) -> f32 {
        let inc = if use_sync { self.sync_inc } else { self.phase_inc };
        let wrapped_before = self.phase;
        self.phase += inc;
        let mut wrapped = false;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            wrapped = true;
        }
        // Per-cycle bookkeeping for S&H / SmoothRandom.
        if wrapped {
            // Pick a new S&H target. The sample value is held until
            // the next wrap.
            self.sh_value = self.next_rand() * 2.0 - 1.0;
            self.sr_target = self.next_rand() * 2.0 - 1.0;
        }
        let _ = wrapped_before;
        match self.shape {
            LfoShape::Sine => fast_sin(self.phase * std::f32::consts::TAU),
            LfoShape::Triangle => {
                if self.phase < 0.5 {
                    self.phase * 4.0 - 1.0
                } else {
                    3.0 - self.phase * 4.0
                }
            }
            LfoShape::Square => if self.phase < 0.5 { 1.0 } else { -1.0 },
            LfoShape::Saw => self.phase * 2.0 - 1.0,
            LfoShape::SampleHold => self.sh_value,
            LfoShape::SmoothRandom => {
                // 1-pole low-pass tracking sr_target. Coefficient
                // chosen so the response time is ~1/3 of one LFO
                // cycle — gives a recognisable "smooth random" feel.
                let coef = 0.001 + inc * 60.0; // rate-dependent
                self.sr_current += (self.sr_target - self.sr_current) * coef.min(0.5);
                self.sr_current
            }
        }
    }

    pub fn phase(&self) -> f32 { self.phase }
}

/// Bhaskara's parabola, refined once: within 0.001 of a sine over the
/// whole cycle.
///
/// The linear term is 4/pi. It was 2/pi, which put a zero where the peak
/// belongs and sent the curve to +-2.45 at the half cycle, where it jumped
/// the full width in one sample. An LFO on a filter's cutoff moved it two
/// octaves between two samples, once per cycle, and that was heard as a
/// jump in a held chord.
#[inline(always)]
fn fast_sin(x: f32) -> f32 {
    const B: f32 = 4.0 / std::f32::consts::PI;
    const C: f32 = 4.0 / (std::f32::consts::PI * std::f32::consts::PI);
    let two_pi = std::f32::consts::TAU;
    let mut p = x - two_pi * (x / two_pi).floor();
    if p > std::f32::consts::PI {
        p -= two_pi;
    }
    let y = B * p - C * p * p.abs();
    0.225 * (y * y.abs() - y) + y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_run_sine_completes_one_cycle() {
        let sr = 48_000.0_f32;
        let mut lfo = SyncedLfo::new(sr);
        lfo.set_rate_hz(1.0);
        lfo.set_shape(LfoShape::Sine);
        // After 1 second at 1 Hz, the LFO should have visited a
        // full cycle (phase returns to ~0, sine ends near 0 after
        // having traversed [+1, -1]).
        let mut min: f32 = 1.0;
        let mut max: f32 = -1.0;
        for _ in 0..(sr as usize) {
            let v = lfo.tick(false);
            min = min.min(v); max = max.max(v);
        }
        assert!(min < -0.9, "expected sine to reach -1, got {min}");
        assert!(max >  0.9, "expected sine to reach +1, got {max}");
        assert!(min >= -1.001 && max <= 1.001, "the sine leaves its range: {min}..{max}");
    }

    /// The sine IS a sine, and it never steps.
    ///
    /// It used to reach 2.45 at the half cycle and jump the whole way to
    /// -2.45 at the next sample. Anything modulated by it jumped with it,
    /// once per cycle, which is what a held note heard.
    #[test]
    fn the_sine_is_one_and_does_not_step() {
        for x in 0..2000 {
            let a = x as f32 / 2000.0 * std::f32::consts::TAU * 3.0 - std::f32::consts::TAU;
            let want = a.sin();
            let got = fast_sin(a);
            assert!((got - want).abs() < 0.002, "sin({a}) = {got}, not {want}");
        }
        let sr = 48_000.0_f32;
        let mut lfo = SyncedLfo::new(sr);
        lfo.set_rate_hz(2.0);
        lfo.set_shape(LfoShape::Sine);
        let mut last = lfo.tick(false);
        let mut worst = 0.0f32;
        for _ in 0..(sr as usize) {
            let v = lfo.tick(false);
            worst = worst.max((v - last).abs());
            last = v;
        }
        // One sample of a 2 Hz sine moves by 2.6e-4 at most.
        assert!(worst < 1e-3, "the sine steps by {worst} in one sample");
    }

    /// Every shape stays inside the range a modulation matrix scales.
    #[test]
    fn every_shape_stays_in_range() {
        let sr = 48_000.0_f32;
        for shape in [LfoShape::Sine, LfoShape::Triangle, LfoShape::Square, LfoShape::Saw, LfoShape::SampleHold, LfoShape::SmoothRandom] {
            let mut lfo = SyncedLfo::new(sr);
            lfo.set_rate_hz(3.0);
            lfo.set_shape(shape);
            let (mut lo, mut hi) = (0.0f32, 0.0f32);
            for _ in 0..(sr as usize) {
                let v = lfo.tick(false);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            assert!(lo >= -1.001 && hi <= 1.001, "{shape:?} runs {lo}..{hi}");
        }
    }

    #[test]
    fn sync_rate_matches_bpm_division() {
        // 120 BPM, 1/4 division → 1 cycle per beat = 2 Hz.
        let sr = 48_000.0_f32;
        let mut lfo = SyncedLfo::new(sr);
        lfo.set_sync(120.0, 2);
        // After 1 second: 2 cycles.
        let mut cycles: i32 = 0;
        let mut last_phase = lfo.phase();
        for _ in 0..(sr as usize) {
            let _ = lfo.tick(true);
            if lfo.phase() < last_phase { cycles += 1; }
            last_phase = lfo.phase();
        }
        assert!((cycles - 2).abs() <= 1, "expected 2 cycles in 1s at 120bpm/quarter, got {cycles}");
    }

    #[test]
    fn retrigger_resets_phase() {
        let sr = 48_000.0_f32;
        let mut lfo = SyncedLfo::new(sr);
        lfo.set_rate_hz(2.0);
        for _ in 0..1000 { let _ = lfo.tick(false); }
        assert!(lfo.phase() > 0.0);
        lfo.retrigger();
        assert_eq!(lfo.phase(), 0.0);
    }

    #[test]
    fn sample_hold_changes_only_on_wrap() {
        let sr = 48_000.0_f32;
        let mut lfo = SyncedLfo::new(sr);
        lfo.set_rate_hz(0.5);
        lfo.set_shape(LfoShape::SampleHold);
        let v0 = lfo.tick(false);
        // Many samples within the same cycle — value should hold.
        let mut held = true;
        for _ in 0..1000 {
            if (lfo.tick(false) - v0).abs() > 1e-6 { held = false; break; }
        }
        assert!(held, "S&H should hold value between wraps");
    }

    #[test]
    fn square_alternates() {
        let sr = 48_000.0_f32;
        let mut lfo = SyncedLfo::new(sr);
        lfo.set_rate_hz(2.0);
        lfo.set_shape(LfoShape::Square);
        let mut seen_pos = false;
        let mut seen_neg = false;
        for _ in 0..(sr as usize) {
            let v = lfo.tick(false);
            if v > 0.5 { seen_pos = true; }
            if v < -0.5 { seen_neg = true; }
        }
        assert!(seen_pos && seen_neg, "square LFO must visit both polarities");
    }
}
