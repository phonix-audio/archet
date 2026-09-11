//! Human vibrato: a multi-component pitch/amplitude vibrato that reads as a
//! living voice rather than a mechanical LFO, shared by every singing voice.
//!
//! Layered on a delayed-onset sine: a slow rate flutter, per-cycle rate+extent
//! wander applied at the phase-0 zero-crossing, a slow warm-up over the note
//! (terminal vibrato), a slightly non-sinusoidal shape, a coupled amplitude
//! vibrato, and a brief attack scoop. The randomness (flutter target + per-cycle
//! wander) is drawn from a caller-supplied RNG closure so the host voice keeps a
//! single deterministic random stream shared with its other jitter/shimmer.

use crate::fastmath::fast_sin;

const TAU: f32 = std::f32::consts::TAU;

/// Per-voice human-vibrato state. Advance one sample with [`HumanVibrato::step`].
///
/// Initial values for a fresh voice: `scoop_t` high (10.0 = scoop already over),
/// `cyc_rate`/`cyc_depth` at 1.0, the rest 0.0; `vib_phase` is typically seeded
/// to a random phase on note-on so section voices are decorrelated.
#[derive(Debug, Clone, Copy)]
pub struct HumanVibrato {
    pub vib_time: f32,
    pub scoop_t: f32,
    pub flutter: f32,
    pub flutter_tgt: f32,
    pub flutter_ctr: u32,
    pub vib_phase: f32,
    pub cyc_rate: f32,
    pub cyc_depth: f32,
}

impl Default for HumanVibrato {
    fn default() -> Self {
        Self {
            vib_time: 0.0, scoop_t: 10.0, flutter: 0.0, flutter_tgt: 0.0,
            flutter_ctr: 0, vib_phase: 0.0, cyc_rate: 1.0, cyc_depth: 1.0,
        }
    }
}

impl HumanVibrato {
    /// Advance one sample and return `(vib, scoop, vib_amp)`:
    /// * `vib`     — pitch modulation in semitones from the vibrato itself.
    /// * `scoop`   — pitch modulation in semitones from the brief attack scoop.
    /// * `vib_amp` — amplitude multiplier (~1.0) coupled to the vibrato depth.
    ///
    /// The caller applies `2^((vib + scoop)/12)` to the pitch and multiplies the
    /// amplitude by `vib_amp`. `rng` must yield uniform values in `[-1, 1]`.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn step(
        &mut self,
        sr: f32,
        vib_delay: f32,
        vib_rate: f32,
        vib_rate_mul: f32,
        vib_depth: f32,
        intensity: f32,
        glide_t: f32,
        glide_dur: f32,
        scoop_amt: f32,
        mut rng: impl FnMut() -> f32,
    ) -> (f32, f32, f32) {
        self.vib_time += 1.0 / sr;
        self.scoop_t += 1.0 / sr;

        // Slow flutter (~2 Hz random drift) — naturalness cue on the rate.
        self.flutter_ctr += 1;
        if self.flutter_ctr >= 256 { self.flutter_ctr = 0; self.flutter_tgt = rng(); }
        self.flutter += (self.flutter_tgt - self.flutter) * 0.02;

        // Evolution: after the bloom the vibrato slowly warms up (a touch faster
        // and wider), like a sustained operatic tone. Caps after ~3 s.
        let evo = {
            let e = ((self.vib_time - vib_delay) / 3.0).clamp(0.0, 1.0);
            e * e * (3.0 - 2.0 * e)
        };

        // Rate: base × per-voice × flutter drift × this-cycle wander × warm-up.
        let vib_rate_eff = vib_rate * vib_rate_mul
            * (1.0 + self.flutter * 0.04) * self.cyc_rate * (1.0 + evo * 0.06);
        self.vib_phase += vib_rate_eff / sr;
        if self.vib_phase >= 1.0 {
            self.vib_phase -= 1.0;
            // New per-cycle wander targets, applied from this zero-crossing on
            // (phase ≈ 0 ⇒ the depth/rate step lands where vib ≈ 0, so smooth).
            self.cyc_rate  = 1.0 + rng() * 0.05; // ±5% rate
            self.cyc_depth = 1.0 + rng() * 0.14; // ±14% extent
        }

        // Depth blooms over ~0.5 s after the delay, then holds.
        let onset = ((self.vib_time - vib_delay) / 0.5).clamp(0.0, 1.0);
        let mut bloom = onset * onset * (3.0 - 2.0 * onset);
        // Sundberg: the vibrato SUSPENDS through a legato transition and
        // re-blooms after -- gliding with full vibrato reads as a slide.
        if glide_t < glide_dur {
            let x = (glide_t / glide_dur).min(1.0);
            bloom *= 1.0 - 0.85 * (std::f32::consts::PI * x).sin();
        }

        // Non-sinusoidal shape: a small phase-shifted 2nd harmonic skews the
        // rise vs fall so the trajectory isn't a perfect (mechanical) sine.
        // Normalised so the peak excursion stays ≈ 1 (vib_depth keeps meaning).
        const VIB_K: f32 = 0.12;
        let p = self.vib_phase * TAU;
        let vib_lfo = (fast_sin(p) + VIB_K * fast_sin(2.0 * p + 1.5)) / (1.0 + VIB_K);

        let depth_eff = vib_depth * (0.45 + 0.55 * intensity) * bloom
            * self.cyc_depth * (1.0 + evo * 0.14);
        let vib = vib_lfo * depth_eff;

        // Amplitude vibrato lags the pitch ~1/8 cycle and is COUPLED to the
        // actual depth, so it vanishes at depth 0 (no constant throb).
        let amp_depth = 0.08 * (depth_eff / 0.5).min(1.0);
        let vib_amp = 1.0 + fast_sin((self.vib_phase + 0.87) * TAU) * amp_depth;

        // Attack scoop: a brief slide up INTO a fresh note (gone within ~120 ms);
        // absent on legato slurs (scoop_t is only reset on a fresh attack).
        let scoop = {
            let s = (self.scoop_t / 0.12).clamp(0.0, 1.0);
            -scoop_amt * (1.0 - s * s * (3.0 - 2.0 * s))
        };

        (vib, scoop, vib_amp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every 1500th sample of the three outputs over half a second at 48 kHz,
    /// with a fixed xorshift rng: covers delay, bloom, several cycles and
    /// wanders. Locks the maths independently of any host engine.
    #[test]
    fn human_vibrato_renders_what_it_rendered() {
        let mut hv = HumanVibrato { vib_phase: 0.13, ..HumanVibrato::default() };
        let mut r: u32 = 0x1234_5678;
        let mut rng = || {
            r ^= r << 13; r ^= r >> 17; r ^= r << 5;
            (r & 0xFFFF) as f32 / 65535.0 * 2.0 - 1.0
        };
        let (mut v, mut s, mut a) = (vec![], vec![], vec![]);
        for _ in 0..24_000 {
            let (vib, scoop, amp) = hv.step(48_000.0, 0.05, 5.5, 1.0, 0.5, 0.8, 1.0, 0.0, 0.2, &mut rng);
            v.push(vib); s.push(scoop); a.push(amp);
        }
        let every = |x: &Vec<f32>| x.iter().step_by(1500).copied().collect::<Vec<_>>();
        assert_close(&every(&v), &[0.0, 0.0, 0.00020789952, -0.0069669383, -0.024025884, 0.0022540258, 0.052310374, 0.073790856, 0.005464962, -0.15542936, -0.15579066, 0.07110586, 0.242511, 0.22949857, -0.04042819, -0.38974398], "vibrato");
        assert_close(&every(&s), &[-0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0, -0.0], "scoop");
        assert_close(&every(&a), &[1.0, 1.0, 1.0001101, 0.99985796, 0.9959952, 0.9935854, 1.0023288, 1.0160117, 1.0149521, 0.99037653, 0.9665471, 0.973218, 1.0205181, 1.0563625, 1.0341549, 0.96971416], "amplitude");
    }

    /// Within 1e-5 of a rendering captured on another machine: libm and FMA
    /// contraction move the last bits, an algorithm change moves far more.
    fn assert_close(got: &[f32], want: &[f32], what: &str) {
        assert_eq!(got.len(), want.len(), "{what}: length");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!((g - w).abs() <= 1e-5, "{what} drifted at sample {i}: {g} vs {w}");
        }
    }

}
