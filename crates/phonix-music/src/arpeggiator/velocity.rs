//! Velocity curves and per-step velocity modulation.

/// Velocity curve types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelocityCurve {
    /// Use input velocity unchanged
    Fixed,
    /// Beat 1 gets accent, others reduced
    AccentOnOne,
    /// Velocity increases across the pattern cycle
    RampUp,
    /// Velocity decreases across the pattern cycle
    RampDown,
    /// Random velocity within ±range
    Random,
    /// Small random variation (subtle feel)
    Humanize,
}

impl VelocityCurve {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Fixed,
            1 => Self::AccentOnOne,
            2 => Self::RampUp,
            3 => Self::RampDown,
            4 => Self::Random,
            _ => Self::Humanize,
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Fixed => 0,
            Self::AccentOnOne => 1,
            Self::RampUp => 2,
            Self::RampDown => 3,
            Self::Random => 4,
            Self::Humanize => 5,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Fixed => "Fixed",
            Self::AccentOnOne => "Accent 1",
            Self::RampUp => "Ramp Up",
            Self::RampDown => "Ramp Down",
            Self::Random => "Random",
            Self::Humanize => "Humanize",
        }
    }

    pub const COUNT: usize = 6;
}

/// Velocity processor — applies curve to base velocity per step.
#[derive(Debug, Clone)]
pub struct VelocityProcessor {
    pub curve: VelocityCurve,
    /// Accent boost amount (0–64)
    pub accent_amount: u8,
    /// ± range for Random/Humanize (0–64)
    pub random_range: u8,
}

impl VelocityProcessor {
    pub fn new() -> Self {
        Self {
            curve: VelocityCurve::Fixed,
            accent_amount: 30,
            random_range: 15,
        }
    }

    /// Compute output velocity for a step.
    /// `base`: input velocity, `step`: current step index, `total`: step count,
    /// `is_accent`: step is accented, `rng`: mutable RNG state.
    pub fn process(
        &self,
        base: u8,
        step: usize,
        total: usize,
        is_accent: bool,
        rng: &mut u32,
    ) -> u8 {
        let total = total.max(1);
        let mut vel = base as i32;

        match self.curve {
            VelocityCurve::Fixed => {}
            VelocityCurve::AccentOnOne => {
                if step % total == 0 {
                    vel += self.accent_amount as i32;
                } else {
                    vel -= (self.accent_amount as i32) / 2;
                }
            }
            VelocityCurve::RampUp => {
                let t = step as f32 / total as f32;
                vel = (vel as f32 * (0.4 + 0.6 * t)) as i32;
            }
            VelocityCurve::RampDown => {
                let t = step as f32 / total as f32;
                vel = (vel as f32 * (1.0 - 0.6 * t)) as i32;
            }
            VelocityCurve::Random => {
                let r = next_rng(rng);
                let offset = (r % (self.random_range as u32 * 2 + 1)) as i32
                    - self.random_range as i32;
                vel += offset;
            }
            VelocityCurve::Humanize => {
                let r = next_rng(rng);
                let range = (self.random_range as u32 / 2).max(1);
                let offset = (r % (range * 2 + 1)) as i32 - range as i32;
                vel += offset;
            }
        }

        // Apply accent boost
        if is_accent {
            vel += self.accent_amount as i32;
        }

        vel.clamp(1, 127) as u8
    }
}

/// Simple LCG random — deterministic, no-alloc
fn next_rng(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1103515245).wrapping_add(12345);
    *state >> 16
}
