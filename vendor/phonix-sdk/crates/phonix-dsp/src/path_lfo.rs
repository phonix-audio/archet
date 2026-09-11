//! Path LFO — a draw-your-own modulation curve (Serum-2 "Path LFO" style).
//!
//! A `PathLfo` is a sequence of breakpoints in `[0,1]` time × `[-1,1]` value with
//! a per-segment tension. `sample(phase)` reads the curve at a phase, so it drops
//! into any LFO / mod-matrix slot in place of a fixed shape — the user draws the
//! modulation instead of picking sine/triangle/square. It is a pure, stateless
//! reader (the caller owns the phase), reusable by any engine.
//!
//! Invariants a well-formed curve keeps (the editing helpers maintain them):
//!   - at least 2 points, sorted by `x`, first `x == 0.0`, last `x == 1.0`;
//!   - the endpoints' `x` are pinned; interior points stay between their
//!     neighbours so the curve never folds back in time.

use serde::{Deserialize, Serialize};

/// One breakpoint on the path. `curve` is the tension of the segment that
/// LEAVES this point toward the next: 0 = linear, >0 eases in (slow start),
/// <0 eases out (fast start).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PathPoint {
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub curve: f32,
}

impl PathPoint {
    pub fn new(x: f32, y: f32) -> Self { Self { x, y, curve: 0.0 } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathLfo {
    /// Breakpoints, kept sorted by `x` with pinned `x==0` / `x==1` endpoints.
    pub points: Vec<PathPoint>,
}

impl Default for PathLfo {
    fn default() -> Self {
        // A smooth up/down triangle — a neutral, obviously-a-shape default.
        Self {
            points: vec![
                PathPoint::new(0.0, -1.0),
                PathPoint::new(0.5, 1.0),
                PathPoint::new(1.0, -1.0),
            ],
        }
    }
}

/// Shape a normalized segment position `t` in `[0,1]` by `curve` tension. `0` is
/// the exact identity (linear); the endpoints `t=0`/`t=1` are always fixed, so a
/// segment stays anchored to its two breakpoints whatever the tension.
#[inline]
fn tension(t: f32, curve: f32) -> f32 {
    if curve.abs() < 1e-4 {
        return t;
    }
    let c = curve.clamp(-1.0, 1.0);
    let exp = if c >= 0.0 { 1.0 + c * 3.0 } else { 1.0 / (1.0 - c * 3.0) };
    t.powf(exp)
}

impl PathLfo {
    /// Read the curve at `phase`. `phase` is wrapped to `[0,1)`, so the caller
    /// can advance a free-running or tempo-synced phase and let this loop.
    /// Returns a value in `[-1, 1]` (assuming point `y` are in range).
    pub fn sample(&self, phase: f32) -> f32 {
        let p = phase - phase.floor();
        let pts = &self.points;
        match pts.len() {
            0 => return 0.0,
            1 => return pts[0].y,
            _ => {}
        }
        // Segment [i, i+1] with pts[i].x <= p < pts[i+1].x (last segment catches p==1-).
        let mut i = 0;
        while i + 1 < pts.len() - 1 && p >= pts[i + 1].x {
            i += 1;
        }
        let a = pts[i];
        let b = pts[i + 1];
        let span = (b.x - a.x).max(1e-6);
        let t = ((p - a.x) / span).clamp(0.0, 1.0);
        a.y + (b.y - a.y) * tension(t, a.curve)
    }

    // ── editing helpers (used by the curve editor) ──────────────────────────

    /// Insert a point, keeping the curve sorted. Endpoint x (0 and 1) are never
    /// created here; an interior x is clamped just inside the range. Returns the
    /// new point's index.
    pub fn add_point(&mut self, x: f32, y: f32) -> usize {
        let x = x.clamp(1e-3, 1.0 - 1e-3);
        let y = y.clamp(-1.0, 1.0);
        let idx = self.points.partition_point(|q| q.x < x);
        self.points.insert(idx, PathPoint::new(x, y));
        idx
    }

    /// Remove an interior point (endpoints are never removed).
    pub fn remove_point(&mut self, i: usize) {
        if i > 0 && i + 1 < self.points.len() {
            self.points.remove(i);
        }
    }

    /// Move point `i`. The two endpoints keep their pinned x (0 / 1) and only
    /// move in y; interior points are clamped strictly between their neighbours.
    pub fn move_point(&mut self, i: usize, x: f32, y: f32) {
        let n = self.points.len();
        if i >= n {
            return;
        }
        let y = y.clamp(-1.0, 1.0);
        if i == 0 {
            self.points[0].y = y; // x pinned at 0
        } else if i == n - 1 {
            self.points[n - 1].y = y; // x pinned at 1
        } else {
            let lo = self.points[i - 1].x + 1e-3;
            let hi = self.points[i + 1].x - 1e-3;
            self.points[i].x = x.clamp(lo, hi);
            self.points[i].y = y;
        }
    }

    /// Set the tension of the segment leaving point `i`.
    pub fn set_curve(&mut self, i: usize, curve: f32) {
        if let Some(p) = self.points.get_mut(i) {
            p.curve = curve.clamp(-1.0, 1.0);
        }
    }

    /// True if this is the neutral default (a caller can early-out on it).
    pub fn is_default(&self) -> bool {
        let d = Self::default();
        self.points == d.points
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_return_the_breakpoint_values_at_their_x() {
        let p = PathLfo::default(); // (0,-1) (0.5,1) (1,-1)
        assert!((p.sample(0.0) - -1.0).abs() < 1e-4);
        assert!((p.sample(0.5) - 1.0).abs() < 1e-4);
        // just before the wrap approaches the last point's value
        assert!((p.sample(0.999) - -1.0).abs() < 0.02);
    }

    #[test]
    fn linear_segment_interpolates_at_the_midpoint() {
        let p = PathLfo::default();
        // midway 0..0.5: -1 -> 1, so at 0.25 it's 0.
        assert!((p.sample(0.25) - 0.0).abs() < 1e-4);
    }

    #[test]
    fn phase_wraps_into_zero_one() {
        let p = PathLfo::default();
        assert!((p.sample(1.25) - p.sample(0.25)).abs() < 1e-6);
        assert!((p.sample(-0.75) - p.sample(0.25)).abs() < 1e-6);
    }

    #[test]
    fn tension_bends_the_segment_but_keeps_the_endpoints() {
        let mut p = PathLfo {
            points: vec![PathPoint::new(0.0, 0.0), PathPoint { x: 1.0, y: 1.0, curve: 0.0 }],
        };
        // linear: midpoint is 0.5
        assert!((p.sample(0.5) - 0.5).abs() < 1e-4);
        p.points[0].curve = 1.0; // ease-in -> below the line mid-way
        assert!(p.sample(0.5) < 0.5);
        // endpoints unchanged
        assert!((p.sample(0.0) - 0.0).abs() < 1e-4);
        assert!((p.sample(0.999) - 1.0).abs() < 0.02);
    }

    #[test]
    fn output_stays_in_range_over_a_grid() {
        let mut p = PathLfo::default();
        p.add_point(0.25, 0.8);
        p.add_point(0.75, -0.6);
        p.set_curve(0, 0.7);
        p.set_curve(2, -0.7);
        for k in 0..200 {
            let v = p.sample(k as f32 / 199.0);
            assert!(v.is_finite() && (-1.0001..=1.0001).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn add_keeps_sorted_and_remove_spares_endpoints() {
        let mut p = PathLfo::default(); // 3 points
        let i = p.add_point(0.3, 0.5);
        assert!(p.points.windows(2).all(|w| w[0].x <= w[1].x), "not sorted");
        assert_eq!(p.points.len(), 4);
        p.remove_point(0); // endpoint: no-op
        p.remove_point(p.points.len() - 1); // endpoint: no-op
        assert_eq!(p.points.len(), 4);
        p.remove_point(i); // interior: removed
        assert_eq!(p.points.len(), 3);
    }

    #[test]
    fn move_pins_endpoint_x_and_clamps_interior() {
        let mut p = PathLfo::default(); // (0,-1)(0.5,1)(1,-1)
        p.move_point(0, 0.4, 0.2); // endpoint: x stays 0
        assert_eq!(p.points[0].x, 0.0);
        assert!((p.points[0].y - 0.2).abs() < 1e-6);
        p.move_point(1, 2.0, 5.0); // interior: x clamped < 1, y clamped <= 1
        assert!(p.points[1].x < 1.0 && p.points[1].x > 0.0);
        assert!((p.points[1].y - 1.0).abs() < 1e-6);
    }
}
