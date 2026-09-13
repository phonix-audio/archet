//! Sympathetic open-string resonators - the fine-instrument "ring".
//!
//! On a real (and especially a fine old Italian) violin the un-played OPEN strings
//! resonate sympathetically with the played notes - strongest when a note matches an
//! open-string harmonic (G3/D4/A4/E5 on a violin) - and keep ringing between and
//! after notes. That persistent halo around the playing is a defining part of the
//! "Stradivarius" glow. Modelled as four damped string loops (feedback comb with
//! fractional delay + loop lowpass, T60 ~1.5 s) per ENGINE, fed by the summed dry
//! output and NEVER reset between notes, so runs leave a ringing aura.

struct Comb {
    buf: Vec<f32>,
    pos: usize,
    delay: f32,   // fractional, samples
    fb: f32,      // loop gain (sets T60)
    fb_held: f32, // loop gain while a finger holds the string: dies within a few ms
    held: bool,
    lp: f32,      // loop lowpass state (string damping: highs die faster)
}

impl Comb {
    fn new(sr: f32, f0: f32, t60: f32) -> Self {
        let delay = sr / f0;
        let len = delay.ceil() as usize + 4;
        let fb = 10f32.powf(-3.0 * delay / (sr * t60));
        // The same order as the plucked voice's own stop under a finger.
        let fb_held = 10f32.powf(-1.0 * delay / (sr * 0.004));
        Self { buf: vec![0.0; len], pos: 0, delay, fb, fb_held, held: false, lp: 0.0 }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let n = self.buf.len() as f32;
        let mut rd = self.pos as f32 - self.delay;
        if rd < 0.0 {
            rd += n;
        }
        let i0 = rd as usize;
        let frac = rd - i0 as f32;
        let i1 = (i0 + 1) % self.buf.len();
        let y = self.buf[i0] * (1.0 - frac) + self.buf[i1] * frac;
        // gentle loop lowpass: upper partials of the sympathetic string decay faster
        self.lp += (y - self.lp) * 0.55;
        // A held string is not free to ring: nothing drives it and what it
        // holds dies under the finger.
        let (drive, fb) = if self.held { (0.0, self.fb_held) } else { (x, self.fb) };
        self.buf[self.pos] = drive + self.lp * fb;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

pub struct SympStrings {
    combs: Vec<Comb>,
    energy: f32,
}

impl SympStrings {
    /// `inst` = body index (0 violin, 1 viola, 2 cello, 3 contrabass).
    ///
    /// Each open string rings for as long as it does when plucked, since it is
    /// the same string: the fundamental of its measured decay table.
    pub fn new(sr: f32, inst: usize) -> Self {
        let open: &[f32] = match inst {
            1 => &[130.81, 196.0, 293.66, 440.0],  // viola C3 G3 D4 A4
            2 => &[65.41, 98.0, 146.83, 220.0],    // cello C2 G2 D3 A3
            3 => &[41.20, 55.0, 73.42, 98.0],      // contrabass E1 A1 D2 G2
            _ => &[196.0, 293.66, 440.0, 659.25],  // violin G3 D4 A4 E5
        };
        let t60 = |string: usize| -> f32 {
            super::voice::ArchetVoice::measured_pizz_t60(inst, string)[0].1
        };
        Self {
            combs: open
                .iter()
                .enumerate()
                .map(|(i, &f)| Comb::new(sr, f, t60(i)))
                .collect(),
            energy: 0.0,
        }
    }

    /// Which open strings a finger or a pluck is on right now: those are not
    /// free to ring in sympathy, being the very strings sounding.
    pub fn set_held(&mut self, held: [bool; 4]) {
        for (c, &h) in self.combs.iter_mut().zip(held.iter()) {
            c.held = h;
        }
    }

    /// Feed the dry engine sum; returns the WET halo only (mix it in at ~-20 dB).
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let drive = x * 0.25; // weak bridge->string coupling
        let mut y = 0.0;
        for c in self.combs.iter_mut() {
            y += c.process(drive);
        }
        self.energy += (y.abs() - self.energy) * 0.0005;
        y
    }

    /// Still ringing? (keeps the engine processing through rests so the halo
    /// doesn't cut off when all voices go idle)
    pub fn active(&self) -> bool {
        self.energy > 1.0e-5
    }
}
