//! Digital-waveguide bowed string (transverse + torsional).
//!
//! The bow divides the string into a long nut-side delay and a short bridge-side
//! delay (split at `beta` = bow-bridge distance / string length). Velocity waves
//! travel in both, reflect inverting at the rigid nut and via a lossy lowpass at
//! the bridge (radiating to the body). A second, faster, low-Q torsional
//! waveguide is co-driven by the bow: the bow contacts the string *surface*, so
//! it sees `v + r*omega`. Torsion desynchronizes stick/slip during the onset,
//! giving a bowed (not "switched-on") attack. (J.O. Smith PASP "Bowed Strings";
//! Bavu/Smith/Wolfe 2005; Woodhouse/Loach 1999.)
//!
//! The friction junction (`super::friction::Friction`) is owned by the voice and
//! passed in each sample; this struct owns only the delay lines and reflection
//! filters.

use super::friction::Friction;

const MAX_DELAY: usize = 4096;

#[inline]
fn read_frac(buf: &[f32], pos: usize, len_frac: f32) -> f32 {
    // Read `len_frac` samples behind the write pointer, linear interpolation.
    let d = len_frac.floor();
    let frac = len_frac - d;
    let di = d as usize;
    let i0 = (pos + MAX_DELAY - di) % MAX_DELAY;
    let i1 = (i0 + MAX_DELAY - 1) % MAX_DELAY;
    buf[i0] * (1.0 - frac) + buf[i1] * frac
}

#[derive(Debug, Clone)]
pub struct BowedWaveguide {
    // Transverse waveguide: nut side (long) + bridge side (short).
    neck: Box<[f32; MAX_DELAY]>,
    bridge: Box<[f32; MAX_DELAY]>,
    neck_pos: usize,
    bridge_pos: usize,
    neck_len: f32,
    bridge_len: f32,

    // Torsional waveguide (single round-trip line, faster, low Q).
    tor: Box<[f32; MAX_DELAY]>,
    tor_pos: usize,
    tor_len: f32,

    // Second (vertical) transverse polarization: a real string vibrates in two
    // planes at slightly different frequencies; their beat is the string's
    // warmth. Modelled as a detuned, more-damped comb fed a tap of the bow
    // injection and summed at `vert_mix`.
    vert: Box<[f32; MAX_DELAY]>,
    vert_pos: usize,
    vert_len: f32,
    vert_lp: f32,
    pub vert_mix: f32,
    pub vert_detune: f32,

    string_lp: f32, // bridge loss one-pole state
    dc_x: f32,      // DC-blocker state (x[n-1]) on the bow injection
    dc_y: f32,      // DC-blocker state (y[n-1])

    // tunables
    pub bow_pos: f32,    // beta
    pub loss: f32,       // bridge loss one-pole coefficient (0=bright .. 1=dark)
    pub tor_ratio: f32,  // torsional/transverse frequency ratio
    pub tor_couple: f32, // how much torsion adds to contact velocity
    pub tor_inject: f32, // how much bow energy enters the torsional line
    pub tor_loss: f32,

    base_total: f32, // transverse round-trip length at base pitch
    sr: f32,
}

impl BowedWaveguide {
    pub fn new(sr: f32) -> Self {
        Self {
            neck: Box::new([0.0; MAX_DELAY]),
            bridge: Box::new([0.0; MAX_DELAY]),
            neck_pos: 0,
            bridge_pos: 0,
            neck_len: 100.0,
            bridge_len: 12.0,
            tor: Box::new([0.0; MAX_DELAY]),
            tor_pos: 0,
            tor_len: 30.0,
            vert: Box::new([0.0; MAX_DELAY]),
            vert_pos: 0,
            vert_len: 160.0,
            vert_lp: 0.0,
            vert_mix: 0.22,
            vert_detune: 0.0025,
            string_lp: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
            bow_pos: 0.13,
            loss: 0.32,
            tor_ratio: 5.0,
            tor_couple: 0.10,
            tor_inject: 0.10,
            tor_loss: 0.55,
            base_total: 160.0,
            sr,
        }
    }

    /// Set delay lengths for a pitch. `bend` is a multiplicative pitch factor
    /// (1.0 = no vibrato/bend) applied to the total length each sample.
    #[inline]
    pub fn set_lengths(&mut self, bend: f32) {
        let total = (self.base_total / bend).clamp(8.0, (MAX_DELAY - 4) as f32);
        self.bridge_len = (total * self.bow_pos).clamp(1.0, total - 2.0);
        self.neck_len = (total - self.bridge_len).max(1.0);
        self.tor_len = (total / self.tor_ratio).clamp(2.0, (MAX_DELAY - 4) as f32);
        // Second polarization: full round-trip comb, detuned slightly flat.
        self.vert_len = (total * (1.0 + self.vert_detune)).clamp(4.0, (MAX_DELAY - 4) as f32);
    }

    pub fn note_on(&mut self, freq: f32) {
        // Round-trip transverse delay = sr/freq, minus the unit delays of the
        // two reflection filters.
        let total = (self.sr / freq - 2.0).clamp(8.0, (MAX_DELAY - 4) as f32);
        self.base_total = total;
        self.set_lengths(1.0);
        self.string_lp = 0.0;

        // Prime both transverse lines with a continuous Helmholtz sawtooth so
        // the note speaks immediately at full level (no organ-like swell), and
        // the read pointer lands on the primed data.
        let blen = self.bridge_len as usize;
        let nlen = self.neck_len as usize;
        let tot = (blen + nlen).max(1);
        // Prime amplitude ~ the steady Helmholtz level so the string starts in
        // equilibrium (no slow swell); the voice's amp_env shapes the soft onset.
        let pa = 1.1f32;
        for i in 0..MAX_DELAY {
            self.bridge[i] = 0.0;
            self.neck[i] = 0.0;
            self.tor[i] = 0.0;
        }
        for i in 0..blen {
            self.bridge[i] = (2.0 * (i as f32 / tot as f32) - 1.0) * pa;
        }
        for i in 0..nlen {
            self.neck[i] = (2.0 * ((i + blen) as f32 / tot as f32) - 1.0) * pa;
        }
        // Prime the second polarization with the same sawtooth (slightly detuned
        // length) so it speaks immediately and beats with the main plane.
        let vlen = self.vert_len as usize;
        for i in 0..MAX_DELAY {
            self.vert[i] = 0.0;
        }
        for i in 0..vlen.min(MAX_DELAY) {
            self.vert[i] = (2.0 * (i as f32 / vlen.max(1) as f32) - 1.0) * pa;
        }
        self.vert_pos = vlen % MAX_DELAY;
        self.vert_lp = 0.0;

        self.bridge_pos = blen % MAX_DELAY;
        self.neck_pos = nlen % MAX_DELAY;
        self.tor_pos = 0;
    }

    /// Legato retune: change the pitch of the ALREADY-RINGING string without
    /// re-priming. The bow keeps going and the existing waves resonate at the new
    /// length -- a same-string finger change (slur), not a new bow stroke.
    pub fn retune(&mut self, freq: f32) {
        let total = (self.sr / freq - 2.0).clamp(8.0, (MAX_DELAY - 4) as f32);
        self.base_total = total;
        self.set_lengths(1.0);
    }

    pub fn reset(&mut self) {
        for i in 0..MAX_DELAY {
            self.neck[i] = 0.0;
            self.bridge[i] = 0.0;
            self.tor[i] = 0.0;
            self.vert[i] = 0.0;
        }
        self.string_lp = 0.0;
        self.vert_lp = 0.0;
    }

    /// Advance one sample. `bow_vel` is the commanded bow velocity (0 when the
    /// bow is lifted), `force` the normalized bow force, `bend` the per-sample
    /// pitch factor (vibrato). Returns the bridge-side output (string force
    /// radiated to the body).
    #[inline]
    pub fn process(&mut self, bow_vel: f32, force: f32, bend: f32, friction: &mut Friction) -> f32 {
        self.set_lengths(bend);

        let bridge_out = read_frac(&self.bridge[..], self.bridge_pos, self.bridge_len);
        let neck_out = read_frac(&self.neck[..], self.neck_pos, self.neck_len);

        // Lossy inverting bridge reflection; inverting rigid nut reflection.
        // Loop gain is coupled to bow force: under the bow (high force) the loop is
        // near-lossless and the bow sustains it; when the bow lifts (force -> 0) the
        // loop loss rises so the string rings DOWN quickly instead of droning on.
        let loop_g = 0.9995 - (1.0 - force.min(1.0)) * 0.03;
        self.string_lp = bridge_out * (1.0 - self.loss) + self.string_lp * self.loss;
        let bridge_refl = -self.string_lp * loop_g;
        let nut_refl = -neck_out * loop_g;
        let trans_vel = bridge_refl + nut_refl;

        // Torsional contribution to contact velocity.
        let tor_out = read_frac(&self.tor[..], self.tor_pos, self.tor_len);
        let string_vel = trans_vel + self.tor_couple * tor_out;

        // Bow friction at the junction.
        let raw_vel = friction.tick(bow_vel, string_vel, force);
        // DC blocker on the injection: the elasto-plastic bristle spring can pump a
        // net DC velocity that the lossless string integrates into a growing offset
        // (centroid collapses to ~0 Hz, clipping). y[n] = x[n]-x[n-1]+R*y[n-1],
        // cutoff ~3 Hz. The static curve is already DC-free, so this passes it through.
        let new_vel = raw_vel - self.dc_x + 0.9996 * self.dc_y;
        self.dc_x = raw_vel;
        self.dc_y = new_vel;

        // Write outgoing waves.
        self.neck[self.neck_pos] = bridge_refl + new_vel;
        self.bridge[self.bridge_pos] = nut_refl + new_vel;
        self.tor[self.tor_pos] = -tor_out * self.tor_loss + new_vel * self.tor_inject;

        // Second (vertical) polarization comb: detuned, a bit more damped, fed a
        // tap of the bow injection. It beats with the main plane -> warmth.
        let vert_out = read_frac(&self.vert[..], self.vert_pos, self.vert_len);
        let vloss = (self.loss + 0.06).min(0.9);
        self.vert_lp = vert_out * (1.0 - vloss) + self.vert_lp * vloss;
        self.vert[self.vert_pos] = -self.vert_lp * loop_g + new_vel * 0.5;

        self.neck_pos = (self.neck_pos + 1) % MAX_DELAY;
        self.bridge_pos = (self.bridge_pos + 1) % MAX_DELAY;
        self.tor_pos = (self.tor_pos + 1) % MAX_DELAY;
        self.vert_pos = (self.vert_pos + 1) % MAX_DELAY;

        bridge_out + vert_out * self.vert_mix
    }
}
