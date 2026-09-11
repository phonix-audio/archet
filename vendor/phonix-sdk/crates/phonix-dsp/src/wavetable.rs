//! A wavetable system.
//!
//! 16 banks × 16 frames × 2048 samples per frame.
//! Banks are procedurally generated at startup from additive synthesis.
//! Each oscillator (FM operator or analog osc) has an independent bank index
//! and scan position, allowing smooth timbral morphing across frames.

use std::f32::consts::TAU;
use std::sync::LazyLock;

pub const NUM_BANKS: usize = 16;
pub const NUM_FRAMES: usize = 16;
pub const FRAME_SIZE: usize = 2048;

pub const BANK_NAMES: [&str; NUM_BANKS] = [
    "Basic",        // 0: sine→saw→pulse→triangle morph
    "Spectral",     // 1: additive harmonic buildup
    "Vocal",        // 2: vowel formants A-E-I-O-U
    "Strings",      // 3: bowed string spectra
    "Keys",         // 4: piano/organ-like
    "Bells",        // 5: inharmonic bell tones
    "Bass",         // 6: fat sub bass
    "Pad",          // 7: soft evolving pad
    "Pluck",        // 8: plucked string spectra
    "Lead",         // 9: classic lead synth
    "Glass",        // 10: glass harmonic
    "Metallic",     // 11: metallic/comb spectra
    "User 1",       // 12-15: empty (sine) slots for future use
    "User 2",
    "User 3",
    "User 4",
];

/// One wavetable bank: 16 frames of 2048 samples each
pub struct WavetableBank {
    pub name: &'static str,
    /// [frame_index][sample_index]
    pub frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]>,
}

pub static WAVETABLE_BANKS: LazyLock<Vec<WavetableBank>> = LazyLock::new(generate_all_banks);

/// Oscillator that reads from a wavetable bank with frame scanning
#[derive(Debug, Clone)]
pub struct WavetableOsc {
    pub bank_idx: usize,
    /// Frame position 0.0..15.0 (continuous, for smooth morph)
    pub scan_pos: f32,
    phase: f32,
    pub sample_rate: f32,
    /// `1.0 / sample_rate`, cached so the per-sample phase advance
    /// is a multiply instead of a division. `sample_rate` is fixed
    /// for the oscillator's lifetime but LLVM can't fold the divide
    /// through the struct-field access, so caching here saves one
    /// hardware FP-div per sample per active wavetable voice.
    inv_sample_rate: f32,
}

impl WavetableOsc {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            bank_idx: 0,
            scan_pos: 0.0,
            phase: 0.0,
            sample_rate,
            inv_sample_rate: 1.0 / sample_rate.max(1.0),
        }
    }

    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    /// Advance phase by dt (= freq/sample_rate), wrapping at 1.0.
    /// Used by FM operators to keep the wavetable osc phase in sync.
    pub fn advance_phase(&mut self, dt: f32) {
        self.phase += dt;
        if self.phase >= 1.0 { self.phase -= 1.0; }
    }

    pub fn current_phase(&self) -> f32 {
        self.phase
    }

    /// Sample at an arbitrary phase (0..1) without advancing the internal phase.
    /// Used by FM operators to apply phase modulation.
    pub fn sample_at_phase(&self, phase: f32) -> f32 {
        let banks = &*WAVETABLE_BANKS;
        if self.bank_idx >= banks.len() {
            return 0.0;
        }
        let bank = &banks[self.bank_idx];
        let scan = self.scan_pos.clamp(0.0, (NUM_FRAMES - 1) as f32);
        let frame_a = scan.floor() as usize;
        let frame_b = (frame_a + 1).min(NUM_FRAMES - 1);
        let blend = scan - frame_a as f32;
        let s_a = read_frame_at(&bank.frames[frame_a], phase);
        let s_b = read_frame_at(&bank.frames[frame_b], phase);
        s_a + blend * (s_b - s_a)
    }

    /// Process one sample at the given frequency (Hz).
    /// Returns a value in roughly -1..+1.
    pub fn process(&mut self, freq: f32) -> f32 {
        let banks = &*WAVETABLE_BANKS;
        if self.bank_idx >= banks.len() {
            return 0.0;
        }
        let bank = &banks[self.bank_idx];

        // Determine which two frames to interpolate
        let scan = self.scan_pos.clamp(0.0, (NUM_FRAMES - 1) as f32);
        let frame_a = scan.floor() as usize;
        let frame_b = (frame_a + 1).min(NUM_FRAMES - 1);
        let blend = scan - frame_a as f32;

        // Read sample from each frame using linear interpolation
        let s_a = read_frame_at(&bank.frames[frame_a], self.phase);
        let s_b = read_frame_at(&bank.frames[frame_b], self.phase);
        let sample = s_a + blend * (s_b - s_a);

        // Advance phase (multiply by cached `1/sr` instead of dividing
        // by `sample_rate` — see field docs).
        let dt = freq * self.inv_sample_rate;
        self.phase += dt;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        if self.phase < 0.0 {
            self.phase += 1.0;
        }

        sample
    }
}

/// Linear interpolation within a single frame (public for use by FM operators)
#[inline(always)]
pub fn read_frame_at(frame: &[f32; FRAME_SIZE], phase: f32) -> f32 {
    let pos = phase.clamp(0.0, 1.0 - f32::EPSILON) * FRAME_SIZE as f32;
    let idx = pos as usize;
    let frac = pos - idx as f32;
    let next = (idx + 1) % FRAME_SIZE;
    frame[idx] * (1.0 - frac) + frame[next] * frac
}

// ─── Bank generation ─────────────────────────────────────────────────────────

fn generate_all_banks() -> Vec<WavetableBank> {
    vec![
        gen_bank_basic(),
        gen_bank_spectral(),
        gen_bank_vocal(),
        gen_bank_strings(),
        gen_bank_keys(),
        gen_bank_bells(),
        gen_bank_bass(),
        gen_bank_pad(),
        gen_bank_pluck(),
        gen_bank_lead(),
        gen_bank_glass(),
        gen_bank_metallic(),
        gen_bank_user("User 1"),
        gen_bank_user("User 2"),
        gen_bank_user("User 3"),
        gen_bank_user("User 4"),
    ]
}

/// Generate a frame from additive synthesis.
/// `harmonics`: list of (harmonic_number, amplitude)
/// Phase offset for odd-even symmetry adjustments.
fn gen_frame_additive(harmonics: &[(usize, f32)]) -> [f32; FRAME_SIZE] {
    let mut frame = [0.0f32; FRAME_SIZE];
    for i in 0..FRAME_SIZE {
        let phase = i as f32 / FRAME_SIZE as f32;
        for &(h, amp) in harmonics {
            frame[i] += amp * (TAU * h as f32 * phase).sin();
        }
    }
    normalize_frame(&mut frame);
    frame
}

/// Normalize frame to peak ±1.0
fn normalize_frame(frame: &mut [f32; FRAME_SIZE]) {
    let max = frame.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
    if max > 0.001 {
        for x in frame.iter_mut() {
            *x /= max;
        }
    }
}

/// Interpolate between two harmonic spectra by t (0..1)
fn lerp_harmonics(a: &[(usize, f32)], b: &[(usize, f32)], t: f32) -> Vec<(usize, f32)> {
    // Collect all unique harmonic numbers
    let mut harmonics: std::collections::HashMap<usize, (f32, f32)> = std::collections::HashMap::new();
    for &(h, amp) in a {
        harmonics.entry(h).or_insert((0.0, 0.0)).0 = amp;
    }
    for &(h, amp) in b {
        harmonics.entry(h).or_insert((0.0, 0.0)).1 = amp;
    }
    harmonics.into_iter()
        .map(|(h, (amp_a, amp_b))| (h, amp_a * (1.0 - t) + amp_b * t))
        .filter(|(_, amp)| amp.abs() > 0.001)
        .collect()
}

// Bank 0: Basic — morph through fundamental waveforms
// Frame 0..3: sine (pure)
// Frame 4..7: sine → saw (add harmonics)
// Frame 8..11: saw
// Frame 12..15: saw → triangle
fn gen_bank_basic() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);

    // Sine
    let sine: Vec<(usize, f32)> = vec![(1, 1.0)];
    // Sawtooth (odd+even harmonics, 1/n)
    let saw: Vec<(usize, f32)> = (1..=32).map(|n| (n, 1.0 / n as f32)).collect();
    // Triangle (odd harmonics, 1/n², alternating sign)
    let tri: Vec<(usize, f32)> = (0..16).map(|k| {
        let n = 2 * k + 1;
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        (n, sign / (n * n) as f32)
    }).collect();
    // Pulse/square (odd harmonics, 1/n)
    let square: Vec<(usize, f32)> = (0..16).map(|k| {
        let n = 2 * k + 1;
        (n, 1.0 / n as f32)
    }).collect();

    // 16 frames: sine(0-3), sine→saw(4-7), saw(8-9), saw→square(10-11), square(12-13), square→tri(14-15)
    let specs: Vec<Vec<(usize, f32)>> = (0..NUM_FRAMES).map(|i| {
        match i {
            0..=2 => sine.clone(),
            3..=5 => lerp_harmonics(&sine, &saw, (i - 3) as f32 / 3.0),
            6..=8 => saw.clone(),
            9..=11 => lerp_harmonics(&saw, &square, (i - 9) as f32 / 3.0),
            12..=13 => square.clone(),
            _ => lerp_harmonics(&square, &tri, (i - 13) as f32 / 2.0),
        }
    }).collect();

    for (i, spec) in specs.iter().enumerate() {
        frames[i] = gen_frame_additive(spec);
    }

    WavetableBank { name: "Basic", frames }
}

// Bank 1: Spectral — harmonic buildup from 1 partial to 16
fn gen_bank_spectral() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    for f in 0..NUM_FRAMES {
        let n_harmonics = f + 1; // 1 to 16 harmonics
        let harmonics: Vec<(usize, f32)> = (1..=n_harmonics).map(|h| (h, 1.0 / h as f32)).collect();
        frames[f] = gen_frame_additive(&harmonics);
    }
    WavetableBank { name: "Spectral", frames }
}

// Bank 2: Vocal — formant-based vowel sounds (A, E, I, O, U and transitions)
fn gen_bank_vocal() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);

    // Simple formant approximation: boost harmonics near formant frequencies
    // Vowel A: F1=800, F2=1200, F3=2500 Hz (approximate relative amplitudes)
    let vowel_a = formant_spectrum(&[(800.0, 1.0, 80.0), (1200.0, 0.7, 100.0), (2500.0, 0.3, 200.0)], 110.0);
    let vowel_e = formant_spectrum(&[(400.0, 1.0, 60.0), (2200.0, 0.8, 150.0), (2800.0, 0.3, 200.0)], 110.0);
    let vowel_i = formant_spectrum(&[(300.0, 1.0, 60.0), (2800.0, 0.9, 200.0), (3300.0, 0.2, 150.0)], 110.0);
    let vowel_o = formant_spectrum(&[(500.0, 1.0, 70.0), (900.0, 0.7, 80.0), (2800.0, 0.2, 200.0)], 110.0);
    let vowel_u = formant_spectrum(&[(300.0, 1.0, 60.0), (900.0, 0.6, 80.0), (2200.0, 0.2, 150.0)], 110.0);

    let vowels = [&vowel_a, &vowel_e, &vowel_i, &vowel_o, &vowel_u];
    for f in 0..NUM_FRAMES {
        let t = f as f32 / (NUM_FRAMES - 1) as f32 * (vowels.len() - 1) as f32;
        let vi = (t.floor() as usize).min(vowels.len() - 2);
        let blend = t - vi as f32;
        let spec = lerp_harmonics(vowels[vi], vowels[vi + 1], blend);
        frames[f] = gen_frame_additive(&spec);
    }
    WavetableBank { name: "Vocal", frames }
}

/// Generate harmonic amplitudes based on formant filter (Gaussian envelope in frequency)
/// formants: [(center_hz, amp, bandwidth_hz)]
fn formant_spectrum(formants: &[(f32, f32, f32)], fundamental_hz: f32) -> Vec<(usize, f32)> {
    let max_harmonic = 32usize;
    let mut result = Vec::new();
    for h in 1..=max_harmonic {
        let freq = h as f32 * fundamental_hz;
        let mut amp = 0.0f32;
        for &(center, formant_amp, bw) in formants {
            let dist = freq - center;
            amp += formant_amp * (-0.5 * (dist / bw).powi(2)).exp();
        }
        if amp > 0.005 {
            result.push((h, amp));
        }
    }
    result
}

// Bank 3: Strings — bowed string spectra (strong odd harmonics, softer evens)
fn gen_bank_strings() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    for f in 0..NUM_FRAMES {
        let brightness = f as f32 / (NUM_FRAMES - 1) as f32; // 0=dark, 1=bright
        let harmonics: Vec<(usize, f32)> = (1..=24).map(|h| {
            let base = 1.0 / h as f32;
            let odd_boost = if h % 2 == 1 { 1.0 + 0.3 * (1.0 - brightness) } else { 1.0 };
            let envelope = (-0.5 * (h as f32 * (1.0 - brightness * 0.8)).powi(2) / 16.0).exp();
            (h, base * odd_boost * envelope)
        }).collect();
        frames[f] = gen_frame_additive(&harmonics);
    }
    WavetableBank { name: "Strings", frames }
}

// Bank 4: Keys — piano/organ morph
fn gen_bank_keys() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    // Piano-like: strong fundamental, harmonics decay quickly
    let piano: Vec<(usize, f32)> = (1..=16).map(|h| {
        (h, (-0.3 * (h - 1) as f32).exp())
    }).collect();
    // Organ-like: even+odd harmonics with specific ratios (Hammond drawbars)
    let organ: Vec<(usize, f32)> = vec![
        (1, 0.8), (2, 1.0), (3, 0.5), (4, 0.6), (6, 0.3), (8, 0.4),
    ];
    for f in 0..NUM_FRAMES {
        let t = f as f32 / (NUM_FRAMES - 1) as f32;
        let spec = lerp_harmonics(&piano, &organ, t);
        frames[f] = gen_frame_additive(&spec);
    }
    WavetableBank { name: "Keys", frames }
}

// Bank 5: Bells — inharmonic partials
fn gen_bank_bells() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    // Bell partials: based on tubular bell ratio approximation
    let bell_ratios: &[f32] = &[1.0, 2.756, 5.404, 8.93, 13.34, 18.64, 24.82, 31.87, 39.79];
    for f in 0..NUM_FRAMES {
        let n_partials = 1 + f * bell_ratios.len() / NUM_FRAMES;
        let mut frame = [0.0f32; FRAME_SIZE];
        for (k, &ratio) in bell_ratios.iter().take(n_partials).enumerate() {
            let amp = (-0.4 * k as f32).exp();
            // fractional harmonic: use continuous phase representation
            for i in 0..FRAME_SIZE {
                let phase = i as f32 / FRAME_SIZE as f32;
                frame[i] += amp * (TAU * ratio * phase).sin();
            }
        }
        normalize_frame(&mut frame);
        frames[f] = frame;
    }
    WavetableBank { name: "Bells", frames }
}

// Bank 6: Bass — fat sub bass with subharmonics feel
fn gen_bank_bass() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    for f in 0..NUM_FRAMES {
        let drive = f as f32 / (NUM_FRAMES - 1) as f32;
        // Strong fundamental + 2nd harmonic, rest fall off steeply
        let harmonics: Vec<(usize, f32)> = (1..=8).map(|h| {
            let amp = match h {
                1 => 1.0,
                2 => 0.5 + drive * 0.4,
                3 => drive * 0.3,
                _ => drive * 0.1 / h as f32,
            };
            (h, amp)
        }).collect();
        frames[f] = gen_frame_additive(&harmonics);
    }
    WavetableBank { name: "Bass", frames }
}

// Bank 7: Pad — soft evolving pad (mainly odd harmonics, gradual)
fn gen_bank_pad() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    for f in 0..NUM_FRAMES {
        let warmth = 1.0 - f as f32 / (NUM_FRAMES - 1) as f32; // warm→bright
        let harmonics: Vec<(usize, f32)> = (1..=12).map(|h| {
            let falloff = (-0.2 * (h - 1) as f32 * (warmth + 0.2)).exp();
            (h, falloff * if h % 2 == 1 { 1.0 } else { 0.4 + warmth * 0.3 })
        }).collect();
        frames[f] = gen_frame_additive(&harmonics);
    }
    WavetableBank { name: "Pad", frames }
}

// Bank 8: Pluck — bright attack decay (many harmonics, rolling off)
fn gen_bank_pluck() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    for f in 0..NUM_FRAMES {
        // Simulate a "snapshot" in time of a plucked string decay
        let decay_stage = f as f32 / (NUM_FRAMES - 1) as f32; // 0=full brightness, 1=nearly decayed
        let harmonics: Vec<(usize, f32)> = (1..=32).map(|h| {
            let amp = 1.0 / h as f32 * (-decay_stage * 0.5 * (h - 1) as f32).exp();
            (h, amp)
        }).collect();
        frames[f] = gen_frame_additive(&harmonics);
    }
    WavetableBank { name: "Pluck", frames }
}

// Bank 9: Lead — classic synth lead shapes
fn gen_bank_lead() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    // Supersaw-like (dense odd harmonics) to thin
    for f in 0..NUM_FRAMES {
        let richness = 1.0 - f as f32 / (NUM_FRAMES - 1) as f32;
        let harmonics: Vec<(usize, f32)> = (1..=16).map(|h| {
            let amp = if h == 1 { 1.0 } else { richness / h as f32 };
            (h, amp)
        }).collect();
        frames[f] = gen_frame_additive(&harmonics);
    }
    WavetableBank { name: "Lead", frames }
}

// Bank 10: Glass — high overtones, crystalline quality
fn gen_bank_glass() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    for f in 0..NUM_FRAMES {
        // Glass: emphasize higher odd harmonics, minimal fundamental
        let peak_h = 3 + f; // shift the peak harmonic upward
        let harmonics: Vec<(usize, f32)> = (1..=20).map(|h| {
            let dist = (h as i32 - peak_h as i32).abs() as f32;
            let amp = (-0.3 * dist * dist).exp();
            (h, amp)
        }).collect();
        frames[f] = gen_frame_additive(&harmonics);
    }
    WavetableBank { name: "Glass", frames }
}

// Bank 11: Metallic — comb-filtered / inharmonic
fn gen_bank_metallic() -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    for f in 0..NUM_FRAMES {
        // Create comb-like spectrum by using non-integer ratios
        let stretch = 1.0 + f as f32 * 0.05; // stretch inharmonicity
        let mut frame = [0.0f32; FRAME_SIZE];
        for k in 1..=12 {
            let ratio = k as f32 * stretch;
            let amp = (-0.15 * (k - 1) as f32).exp();
            for i in 0..FRAME_SIZE {
                let phase = i as f32 / FRAME_SIZE as f32;
                frame[i] += amp * (TAU * ratio * phase).sin();
            }
        }
        normalize_frame(&mut frame);
        frames[f] = frame;
    }
    WavetableBank { name: "Metallic", frames }
}

// Banks 12-15: User banks — just sine for now
fn gen_bank_user(name: &'static str) -> WavetableBank {
    let mut frames: Box<[[f32; FRAME_SIZE]; NUM_FRAMES]> = Box::new([[0.0; FRAME_SIZE]; NUM_FRAMES]);
    let sine_frame = gen_frame_additive(&[(1, 1.0)]);
    for f in 0..NUM_FRAMES {
        frames[f] = sine_frame;
    }
    WavetableBank { name, frames }
}
