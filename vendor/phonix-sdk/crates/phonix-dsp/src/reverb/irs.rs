//! Impulse-response library for convolution reverb, rendered procedurally at
//! first use: sparse Gaussian early reflections, a diffuse exponentially
//! decaying tail, per-channel decorrelation, an HF damping shelf in the
//! decay. `ImpulseResponse` is opaque float data, so a recorded IR fits the
//! same shape.

use std::f32::consts::TAU;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IrKind {
    Cathedral       = 0,
    ConcreteChamber = 1,
    Plate           = 2,
    WoodenHall      = 3,
    Spring          = 4,
    AmbientRoom     = 5,
    ConcertHall     = 6,
}

impl IrKind {
    pub const ALL: &'static [IrKind] = &[
        IrKind::Cathedral, IrKind::ConcreteChamber, IrKind::Plate,
        IrKind::WoodenHall, IrKind::Spring, IrKind::AmbientRoom,
        IrKind::ConcertHall,
    ];

    /// Per-IR character target for an algorithmic reverb. Returns
    /// `(target_size, target_damping)`:
    /// - **target_size** (0..1) — tail length bias (Cathedral=long
    ///   bright stone, Plate=short, Spring=very short, Room=small)
    /// - **target_damping** (0..1) — high-frequency loss per echo
    ///   (Cathedral/Plate/Spring=bright, Wooden Hall/Ambient=dark)
    ///
    /// One shared helper, so every algorithmic branch gets the same
    /// IR-aware character.
    pub fn algorithmic_target(self) -> (f32, f32) {
        match self {
            IrKind::Cathedral       => (0.95, 0.05),
            IrKind::ConcreteChamber => (0.75, 0.45),
            IrKind::Plate           => (0.55, 0.05),
            IrKind::WoodenHall      => (0.75, 0.80),
            IrKind::Spring          => (0.35, 0.05),
            IrKind::AmbientRoom     => (0.50, 0.70),
            // Big, but nothing like a cathedral, and warm rather than damped:
            // a hall's tail loses its top to air and to the audience, not to
            // soft walls.
            IrKind::ConcertHall     => (0.82, 0.55),
        }
    }

    /// Blend the user's "size" knob (0..1) with this IR's character
    /// size, then return `(effective_size, target_damping)` ready
    /// for `set_size` / `set_damping` calls on the algorithmic
    /// reverb. The formula:
    /// `size = 0.5 * knob + 0.5 * target_size`.
    pub fn algorithmic_params(self, knob: f32) -> (f32, f32) {
        let (target_size, damp) = self.algorithmic_target();
        let size = (0.5 * knob + 0.5 * target_size).clamp(0.0, 1.0);
        (size, damp)
    }

    pub fn label(self) -> &'static str {
        match self {
            IrKind::Cathedral       => "Cathedral 8s",
            IrKind::ConcreteChamber => "Concrete 4s",
            IrKind::Plate           => "Plate 3s",
            IrKind::WoodenHall      => "Wooden Hall 5s",
            IrKind::Spring          => "Spring 2s",
            IrKind::AmbientRoom     => "Ambient 1s",
            IrKind::ConcertHall     => "Concert Hall 2s",
        }
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL.get(i).copied().unwrap_or(IrKind::AmbientRoom)
    }

    pub fn index(&self) -> usize {
        Self::ALL.iter().position(|&k| k == *self).unwrap_or(0)
    }
}

/// One stereo IR. `left` / `right` are independent arrays so the IR
/// has true L/R decorrelation (mono IRs feel narrow).
pub struct ImpulseResponse {
    pub kind:        IrKind,
    pub sample_rate: f32,
    pub left:        Vec<f32>,
    pub right:       Vec<f32>,
}

pub struct IrLibrary {
    pub irs: Vec<ImpulseResponse>,
}

impl IrLibrary {
    pub fn get(&self, kind: IrKind) -> Option<&ImpulseResponse> {
        self.irs.iter().find(|ir| ir.kind == kind)
    }
}

pub const IR_SAMPLE_RATE: f32 = 22_050.0;

static LIB: OnceLock<IrLibrary> = OnceLock::new();

pub fn shared() -> &'static IrLibrary {
    LIB.get_or_init(|| IrLibrary {
        irs: IrKind::ALL.iter().map(|&k| generate_ir(k)).collect(),
    })
}

fn generate_ir(kind: IrKind) -> ImpulseResponse {
    // (length_sec, decay_tau, hf_damp_coef, er_count, er_min_ms, er_max_ms, brightness)
    //
    // `er_min_ms` is the gap before the FIRST reflection arrives — what room
    // acoustics calls the initial time delay gap. It is 2 ms for everything that
    // was here before, which is what the generator always used, so those six are
    // unchanged to the sample and no session that uses them shifts.
    let (len_sec, tau, hf_damp, er_count, er_min_ms, er_max_ms, bright) = match kind {
        // tau lower = decays faster; chosen to give the named "X-second" tail.
        IrKind::Cathedral       => (7.5, 1.8, 0.55, 14, 2.0, 90.0,  0.65),
        IrKind::ConcreteChamber => (3.5, 0.9, 0.40, 18, 2.0, 50.0,  0.80),
        IrKind::Plate           => (2.8, 0.7, 0.30, 28, 2.0, 25.0,  0.92),
        IrKind::WoodenHall      => (4.5, 1.2, 0.50, 16, 2.0, 70.0,  0.55),
        IrKind::Spring          => (1.8, 0.5, 0.20, 12, 2.0, 30.0,  0.95),
        IrKind::AmbientRoom     => (0.9, 0.35, 0.35, 22, 2.0, 25.0, 0.75),
        // A concert hall, which none of the above is: the cathedral rings three
        // times too long, the wooden hall is a large church, and the ambient room
        // is a room. Built to published hall acoustics rather than to taste:
        //
        // * `tau` 0.29 puts the reverberation time at 6.9*tau = 2.0 s, the middle
        //   of Beranek's 1.8-2.2 s for an occupied symphonic hall.
        // * The first reflection is held off until 18 ms. That gap is the single
        //   most characteristic thing about a good hall — Beranek's measure of
        //   "intimacy", 15 to 35 ms in the halls that are admired, and about 15 ms
        //   in Boston Symphony Hall. A room answers in 2 ms; a hall makes you wait.
        // * Few, strong, well-separated early reflections, as a shoebox gives from
        //   its side walls, rather than the dense scatter of a chamber.
        // * The tail is allowed to finish inside the IR instead of being cut off
        //   partway down, which is what every one of the six above does.
        IrKind::ConcertHall     => (2.8, 0.29, 0.45, 12, 18.0, 55.0, 0.62),
    };
    // How the reverberation time itself varies with frequency, as a multiple of
    // `tau`. A tail that decays at one rate across the spectrum is the single
    // most synthetic thing about a generated reverb: a real room does not do it.
    // Low frequencies live longer — Beranek's "bass ratio", 1.1 to 1.25 in the
    // halls that are admired — and the top is eaten by the air itself and by the
    // audience, which is why a hall sounds warmer the longer you listen to one
    // note. Everything that was here before keeps 1.0 and 1.0, so those six IRs
    // are unchanged to the sample and no saved session shifts under them.
    let (bass_ratio, hf_ratio) = match kind {
        IrKind::ConcertHall => (1.18f32, 0.62f32),
        _ => (1.0, 1.0),
    };
    let sr = IR_SAMPLE_RATE;
    let n  = (len_sec * sr) as usize;
    let mut left  = vec![0.0_f32; n];
    let mut right = vec![0.0_f32; n];
    // Two independent LCG streams for L/R decorrelation.
    let mut rng_l: u64 = 0x9E37_79B9_7F4A_7C15_u64 ^ ((kind as u64) << 32) ^ 0xAAAA_BBBB;
    let mut rng_r: u64 = 0xC0DE_F00D_DEAD_BEEF_u64 ^ ((kind as u64) << 32) ^ 0xCCCC_DDDD;
    // 1-pole LP states for HF damping in the tail.
    let mut lp_l = 0.0_f32;
    let mut lp_r = 0.0_f32;
    let cutoff_g = 1.0 - (-TAU * 1000.0 * bright / sr).exp();

    // Direct signal: small spike at sample 0, scaled to avoid hot
    // (real IRs have a normalized direct component).
    left[0]  = 0.5;
    right[0] = 0.5;

    // Early reflections — sparse Gaussian-distributed delays in
    // [2 ms, er_max_ms]. Use rng_l for L, rng_r for R so they're
    // independent. Amplitudes shaped so first ER ~0.5, dropping ~0.05
    // per reflection.
    for i in 0..er_count {
        let amp = (0.55 - (i as f32) * 0.03).max(0.05);
        let d_l = (next_unit(&mut rng_l) * (er_max_ms - er_min_ms) + er_min_ms) * 0.001 * sr;
        let d_r = (next_unit(&mut rng_r) * (er_max_ms - er_min_ms) + er_min_ms) * 0.001 * sr;
        let idx_l = d_l as usize;
        let idx_r = d_r as usize;
        let sign_l = if (rng_l >> 60) & 1 == 0 { 1.0 } else { -1.0 };
        let sign_r = if (rng_r >> 60) & 1 == 0 { 1.0 } else { -1.0 };
        if idx_l < n { left[idx_l]  += amp * sign_l; }
        if idx_r < n { right[idx_r] += amp * sign_r; }
    }

    // Late tail: exponentially-decaying gaussian noise with HF damp.
    // Predelay to skip direct + ER region.
    let predelay = (er_max_ms * 0.001 * sr) as usize + (0.020 * sr) as usize;
    let plain = bass_ratio == 1.0 && hf_ratio == 1.0;
    // Band splitters for the three-rate tail. 250 Hz and 2 kHz are where a room
    // is measured: below the first the bass ratio is quoted, above the second is
    // where air absorption takes over.
    let g_lo = 1.0 - (-TAU * 250.0 / sr).exp();
    let g_hi = 1.0 - (-TAU * 2000.0 / sr).exp();
    let (mut lo_l, mut lo_r, mut hi_l, mut hi_r) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for i in predelay..n {
        let t = (i - predelay) as f32 / sr;
        let env = (-t / tau).exp();
        let wl = (next_unit(&mut rng_l) - 0.5) * 2.0;
        let wr = (next_unit(&mut rng_r) - 0.5) * 2.0;
        // Damp HF in the late portion based on hf_damp factor: the
        // larger `hf_damp`, the more HF cut for "warmer" tails.
        let damp_g = cutoff_g * (1.0 - hf_damp * 0.6);
        lp_l += damp_g * (wl - lp_l);
        lp_r += damp_g * (wr - lp_r);
        if plain {
            left[i] += lp_l * env;
            right[i] += lp_r * env;
            continue;
        }
        // Split what the damping left into three bands and let each decay at its
        // own rate, then put them back together.
        lo_l += g_lo * (lp_l - lo_l);
        lo_r += g_lo * (lp_r - lo_r);
        hi_l += g_hi * (lp_l - hi_l);
        hi_r += g_hi * (lp_r - hi_r);
        let (mid_l, mid_r) = (hi_l - lo_l, hi_r - lo_r);
        let (top_l, top_r) = (lp_l - hi_l, lp_r - hi_r);
        let env_lo = (-t / (tau * bass_ratio)).exp();
        let env_hi = (-t / (tau * hf_ratio)).exp();
        left[i] += lo_l * env_lo + mid_l * env + top_l * env_hi;
        right[i] += lo_r * env_lo + mid_r * env + top_r * env_hi;
    }

    // Normalize each channel to a sane peak (~0.7) — convolution
    // sums many of these so we keep headroom.
    normalize(&mut left, 0.7);
    normalize(&mut right, 0.7);

    ImpulseResponse { kind, sample_rate: sr, left, right }
}

#[cfg(test)]
mod hall_tests {
    use super::*;

    /// The concert hall has to BE a concert hall, by the numbers room acoustics
    /// uses to describe one, not by the name on the label.
    #[test]
    fn the_concert_hall_measures_like_one() {
        let lib = shared();
        let ir = lib.get(IrKind::ConcertHall).expect("the hall must be in the library");
        let sr = ir.sample_rate;
        let x = &ir.left;

        // The initial time delay gap: how long before the room answers. Beranek
        // made this his measure of intimacy — 15 to 35 ms in the halls players
        // and audiences prefer. Find the first reflection after the direct spike.
        let direct = x[0].abs();
        let mut first = 0usize;
        let _ = &x;
        for (i, v) in x.iter().enumerate().skip(1) {
            if v.abs() > direct * 0.15 {
                first = i;
                break;
            }
        }
        let itdg_ms = 1000.0 * first as f32 / sr;
        // Reverberation time, from the energy decay of the tail.
        let energy_after = |t: f32| -> f32 {
            let i = (t * sr) as usize;
            x[i.min(x.len())..].iter().map(|v| v * v).sum::<f32>()
        };
        let e0 = energy_after(0.10);
        let mut rt60 = 0.0f32;
        let mut t = 0.10f32;
        while t < x.len() as f32 / sr {
            if 10.0 * (energy_after(t) / e0).max(1e-30).log10() < -30.0 {
                // -30 dB of the REMAINING energy, doubled, is the usual T30
                // extrapolation to a full 60 dB.
                rt60 = (t - 0.10) * 2.0;
                break;
            }
            t += 0.01;
        }
        eprintln!("salle de concert : ITDG {itdg_ms:.1} ms, TR {rt60:.2} s");
        assert!(
            (12.0..=40.0).contains(&itdg_ms),
            "the room answers in {itdg_ms:.1} ms — that is a room, not a hall"
        );
        assert!(
            (1.4..=2.6).contains(&rt60),
            "reverberation time {rt60:.2} s is outside what a concert hall does"
        );

        // And it must be shorter than the cathedral it replaces, or nothing was
        // gained.
        let cath = lib.get(IrKind::Cathedral).expect("cathedral");
        assert!(
            ir.left.len() < cath.left.len(),
            "the hall is no shorter than the cathedral"
        );
    }

    /// A hall must not decay at the same rate at every frequency.
    ///
    /// One rate across the spectrum is the most synthetic thing a generated
    /// reverb does. A real room keeps its bass longer than its middle, and loses
    /// its top to the air and the audience — measured here by comparing how much
    /// energy each band still has late in the tail against what it had early.
    #[test]
    fn the_hall_decays_at_different_rates_per_band() {
        let lib = shared();
        let ir = lib.get(IrKind::ConcertHall).expect("hall");
        let sr = ir.sample_rate;
        let x = &ir.left;
        // Energy in a band over a span, by one-pole splitting — good enough to
        // compare a band with ITSELF at two instants, which is all this asks.
        let band_energy = |lo: f32, hi: f32, from: f32, to: f32| -> f64 {
            let (a, b) = ((from * sr) as usize, ((to * sr) as usize).min(x.len()));
            if a >= b {
                return 0.0;
            }
            let glo = 1.0 - (-TAU * lo / sr).exp();
            let ghi = 1.0 - (-TAU * hi / sr).exp();
            let (mut l, mut h) = (0.0f32, 0.0f32);
            let mut acc = 0.0f64;
            for (i, s) in x.iter().enumerate().take(b) {
                l += glo * (s - l);
                h += ghi * (s - h);
                if i >= a {
                    let v = (h - l) as f64;
                    acc += v * v;
                }
            }
            acc
        };
        let decay = |lo: f32, hi: f32| -> f64 {
            let early = band_energy(lo, hi, 0.10, 0.35);
            let late = band_energy(lo, hi, 0.80, 1.05);
            10.0 * ((late + 1e-30) / (early + 1e-30)).log10()
        };
        let bass = decay(60.0, 250.0);
        let mid = decay(400.0, 1500.0);
        let top = decay(3000.0, 8000.0);
        eprintln!("salle, perte entre 0.2 s et 0.9 s : grave {bass:.1} dB, medium {mid:.1}, aigu {top:.1}");
        assert!(bass > mid + 1.0, "the bass does not outlast the middle ({bass:.1} vs {mid:.1})");
        assert!(top < mid - 1.0, "the top does not fade before the middle ({top:.1} vs {mid:.1})");
    }

    /// Adding a kind must not have moved the six that were already there: every
    /// saved session picks its room by index.
    #[test]
    fn the_older_rooms_keep_their_places() {
        assert_eq!(IrKind::from_index(0), IrKind::Cathedral);
        assert_eq!(IrKind::from_index(3), IrKind::WoodenHall);
        assert_eq!(IrKind::from_index(5), IrKind::AmbientRoom);
        assert_eq!(IrKind::ConcertHall.index(), 6);
    }
}

fn normalize(buf: &mut [f32], target_peak: f32) {
    let peak = buf.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
    if peak < 1e-6 { return; }
    let g = target_peak / peak;
    for s in buf.iter_mut() { *s *= g; }
}

#[inline(always)]
fn next_unit(rng: &mut u64) -> f32 {
    *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*rng >> 33) as f32) / (1u64 << 31) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_renders_every_ir() {
        let lib = shared();
        // Against the catalogue, not against a number somebody has to remember
        // to bump — this failed the moment a seventh room was added.
        assert_eq!(lib.irs.len(), IrKind::ALL.len());
        for ir in &lib.irs {
            assert!(ir.left.len() > 1000, "IR {:?} too short", ir.kind);
            assert_eq!(ir.left.len(), ir.right.len());
            let peak_l = ir.left.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
            let peak_r = ir.right.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
            assert!(peak_l > 0.5 && peak_l <= 1.0);
            assert!(peak_r > 0.5 && peak_r <= 1.0);
        }
    }

    #[test]
    fn ir_lengths_match_named_durations() {
        let lib = shared();
        let durs_secs: Vec<f32> = lib.irs.iter()
            .map(|ir| ir.left.len() as f32 / ir.sample_rate)
            .collect();
        // Cathedral should be the longest, AmbientRoom shortest.
        let max_idx = durs_secs.iter()
            .enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert_eq!(lib.irs[max_idx].kind, IrKind::Cathedral);
        let min_idx = durs_secs.iter()
            .enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
        assert_eq!(lib.irs[min_idx].kind, IrKind::AmbientRoom);
    }

    #[test]
    fn left_right_are_decorrelated_in_late_tail() {
        let lib = shared();
        for ir in &lib.irs {
            // Direct signal + early reflections are sparse spikes;
            // most positions are 0 in both channels there, which
            // would falsely read as correlated. Inspect a slice of
            // the late tail (1000 samples starting 5% into the IR)
            // where both channels carry decorrelated noise.
            let n = ir.left.len();
            let start = (n as f32 * 0.05) as usize;
            let end   = (start + 2000).min(n);
            let mut diffs = 0;
            for i in start..end {
                if (ir.left[i] - ir.right[i]).abs() > 1e-4 { diffs += 1; }
            }
            assert!(diffs > 500,
                "IR {:?} L and R appear correlated in late tail ({} diffs in {} samples)",
                ir.kind, diffs, end - start);
        }
    }
}
