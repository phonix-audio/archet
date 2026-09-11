//! Built-in sidechain-style amp ducker.
//!
//! A tempo-synced AR envelope on amp gain that drops to `1 - depth` on
//! each beat boundary and recovers toward 1.0 over `release_ms`. No
//! external sidechain input: the engine's own step clock is the trigger.

/// Persisted ducker config, part of an engine's patch.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DuckerConfig {
    pub enabled:     bool,
    /// Maximum ducking depth, 0..1.
    /// 0 = no ducking, 1 = signal completely silenced at trigger.
    pub depth:       f32,
    /// Attack time in ms — how long to reach maximum ducking after
    /// a trigger. Typical 1..20 ms for tight pumping; longer values
    /// give a "breath" feel.
    pub attack_ms:   f32,
    /// Release time in ms — how long to recover from ducking back
    /// to unity gain. Typical 80..400 ms.
    pub release_ms:  f32,
    /// Tempo division for the trigger: 0=1/4, 1=1/8, 2=1/16, 3=1/2,
    /// 4=1 bar. Counted in the engine's 16-steps-per-bar grid.
    pub sync_div:    u8,
}

impl Default for DuckerConfig {
    fn default() -> Self {
        Self {
            enabled:     false,
            depth:       0.6,
            attack_ms:   3.0,
            release_ms:  220.0,
            sync_div:    0, // 1/4
        }
    }
}

impl DuckerConfig {
    /// Step interval (in 16-steps-per-bar units) between triggers
    /// for the configured division.
    pub fn trigger_period_steps(self) -> u32 {
        match self.sync_div {
            0 => 4,   // 1/4
            1 => 2,   // 1/8
            2 => 1,   // 1/16
            3 => 8,   // 1/2
            _ => 16,  // 1 bar (and anything out of range)
        }
    }
}

/// Per-voice runtime ducker. Holds the AR envelope state and
/// produces a gain multiplier in `tick`. Pure DSP — no allocation,
/// no branching beyond the attack/release direction switch.
pub struct Ducker {
    sr:           f32,
    /// Current envelope level. 1.0 = unity gain, 0.0 = fully open
    /// for ducking (so the output is multiplied by `1 - depth`).
    level:        f32,
    /// Target the level is glide-toward. Set to 0.0 on trigger
    /// (initiates the duck) and back to 1.0 once the level has
    /// reached the floor (initiates the release).
    target:       f32,
    /// Are we currently in the attack phase of the duck? Once the
    /// level reaches the floor (~0.0) we switch to release.
    in_attack:    bool,
    /// Cached coefficients — recomputed when params change.
    attack_coef:  f32,
    release_coef: f32,
}

impl Ducker {
    pub fn new(sr: f32) -> Self {
        let mut d = Self {
            sr,
            level: 1.0,
            target: 1.0,
            in_attack: false,
            attack_coef: 0.0,
            release_coef: 0.0,
        };
        d.set_times(3.0, 220.0);
        d
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
    }

    /// Recompute the 1-pole coefficients from attack/release times.
    pub fn set_times(&mut self, attack_ms: f32, release_ms: f32) {
        let a = (attack_ms  * 0.001 * self.sr).max(1.0);
        let r = (release_ms * 0.001 * self.sr).max(1.0);
        // 1 - exp(-1/N) gives the per-sample mix factor that reaches
        // ~63% of the target in N samples.
        self.attack_coef  = 1.0 - (-1.0 / a).exp();
        self.release_coef = 1.0 - (-1.0 / r).exp();
    }

    /// Trigger a duck — pulls level toward 0 with the attack coef.
    pub fn trigger(&mut self) {
        self.target = 0.0;
        self.in_attack = true;
    }

    /// Reset to unity gain. Used on patch reload / loop wrap.
    pub fn reset(&mut self) {
        self.level = 1.0;
        self.target = 1.0;
        self.in_attack = false;
    }

    /// True when the envelope has fully released to unity gain — used
    /// by host engines to skip per-sample `tick()` when no duck is in
    /// flight (the dominant cost is the multiply-add inside, but the
    /// call itself isn't free either when called per audio sample per
    /// track).
    #[inline(always)]
    pub fn is_idle(&self) -> bool {
        !self.in_attack && (self.level - 1.0).abs() < 1.0e-4
    }

    /// Advance one sample, returning the gain multiplier the engine
    /// should apply to the voice output. `depth` is read live so
    /// the GUI knob is immediately audible without command spam.
    #[inline(always)]
    pub fn tick(&mut self, depth: f32) -> f32 {
        let coef = if self.in_attack { self.attack_coef } else { self.release_coef };
        self.level += (self.target - self.level) * coef;
        // Once the level has hit the floor switch to release.
        if self.in_attack && self.level < 0.01 {
            self.in_attack = false;
            self.target = 1.0;
        }
        // level=1.0 → gain 1.0; level=0.0 → gain (1 - depth).
        1.0 - depth * (1.0 - self.level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_ducks_then_releases() {
        let sr = 48_000.0;
        let mut d = Ducker::new(sr);
        d.set_times(2.0, 100.0);
        d.trigger();
        // Run for 5 ms — should be well into the duck.
        let mut min_gain: f32 = 1.0;
        for _ in 0..(sr as usize * 5 / 1000) {
            let g = d.tick(0.8);
            if g < min_gain { min_gain = g; }
        }
        // depth=0.8 → minimum gain ~0.2 (= 1 - 0.8). Allow a small
        // margin for the attack coef shape.
        assert!(min_gain < 0.3,
                "duck didn't drop deeply: min_gain={min_gain:.3} (depth=0.8 → expect <0.3)");

        // Run for 300 ms — should fully recover.
        let mut last_gain = 0.0;
        for _ in 0..(sr as usize * 300 / 1000) {
            last_gain = d.tick(0.8);
        }
        assert!(last_gain > 0.95,
                "duck didn't release: last_gain={last_gain:.3} (expect > 0.95)");
    }

    #[test]
    fn trigger_period_matches_division() {
        let mut c = DuckerConfig::default();
        c.sync_div = 0; assert_eq!(c.trigger_period_steps(), 4);  // 1/4
        c.sync_div = 1; assert_eq!(c.trigger_period_steps(), 2);  // 1/8
        c.sync_div = 2; assert_eq!(c.trigger_period_steps(), 1);  // 1/16
        c.sync_div = 3; assert_eq!(c.trigger_period_steps(), 8);  // 1/2
        c.sync_div = 4; assert_eq!(c.trigger_period_steps(), 16); // bar
    }
}
