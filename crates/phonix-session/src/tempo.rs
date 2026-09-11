//! Tempo and meter along the arrangement.
//!
//! The session's `bpm` and `time_sig` are what holds at tick zero. A tempo
//! point after it sets a new tempo, held until the next point or ramped
//! linearly up to it; a meter change sets a new time signature from the bar
//! it stands on. Patterns keep their length in bars of the base meter: a
//! meter change moves the grid, the ruler and the bar count, not the notes.

use serde::{Deserialize, Serialize};

use crate::data::{bar_ticks_of, Session, PPQN};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct TempoPoint {
    pub tick: u64,
    pub bpm:  f32,
    /// Ramp linearly to the next point rather than hold.
    #[serde(default)]
    pub ramp: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct MeterChange {
    /// Where the change stands: the start of a bar in the meter before it.
    pub tick: u64,
    pub num:  u8,
    pub den:  u8,
}

impl MeterChange {
    pub fn sig(&self) -> (u8, u8) {
        (self.num.max(1), self.den.max(1))
    }
}

pub const MIN_BPM: f32 = 20.0;
pub const MAX_BPM: f32 = 300.0;

impl Session {
    /// Put a tempo map in order: sorted, one point per tick, a point at zero
    /// becomes the base tempo, every tempo inside its range.
    pub fn set_tempo_map(&mut self, mut points: Vec<TempoPoint>) {
        points.sort_by_key(|p| p.tick);
        points.dedup_by_key(|p| p.tick);
        for p in &mut points {
            p.bpm = p.bpm.clamp(MIN_BPM, MAX_BPM);
        }
        if let Some(first) = points.first() {
            if first.tick == 0 {
                self.bpm = first.bpm;
                points.remove(0);
            }
        }
        self.tempo_map = points;
    }

    /// Put a meter map in order: sorted, one change per tick, each on a bar
    /// line of the meter before it, a change at zero becomes the base meter.
    pub fn set_meter_map(&mut self, mut changes: Vec<MeterChange>) {
        changes.sort_by_key(|c| c.tick);
        changes.dedup_by_key(|c| c.tick);
        if let Some(first) = changes.first() {
            if first.tick == 0 {
                self.time_sig = first.sig();
                changes.remove(0);
            }
        }
        self.meter_map.clear();
        for c in changes {
            let (_, start) = self.bar_start_of(c.tick);
            if self.meter_map.last().map_or(true, |l| l.tick < start) {
                self.meter_map.push(MeterChange { tick: start, num: c.num.max(1), den: c.den.max(1) });
            }
        }
    }

    /// The tempo in force at `tick`.
    pub fn tempo_at(&self, tick: u64) -> f32 {
        let (mut t0, mut b0, mut ramp) = (0u64, self.bpm, false);
        for p in &self.tempo_map {
            if p.tick > tick {
                if ramp && p.tick > t0 {
                    let u = (tick - t0) as f32 / (p.tick - t0) as f32;
                    return b0 + (p.bpm - b0) * u;
                }
                return b0;
            }
            t0 = p.tick;
            b0 = p.bpm;
            ramp = p.ramp;
        }
        b0
    }

    /// The tempo map that makes `target_tick` arrive `secs` after tick
    /// zero: the tempo in force before the target is replaced, held, and
    /// the tempo that was at the target goes on from there. None when the
    /// moment is not after the point before the target.
    pub fn tempo_map_landing(&self, secs: f64, target_tick: u64) -> Option<Vec<TempoPoint>> {
        let (mut t_prev, mut b_prev) = (0u64, self.bpm);
        for p in &self.tempo_map {
            if p.tick < target_tick {
                t_prev = p.tick;
                b_prev = p.bpm;
            }
        }
        let s_prev = self.tick_to_seconds(t_prev);
        if secs <= s_prev || target_tick <= t_prev {
            return None;
        }
        let bpm = ((target_tick - t_prev) as f64 * 60.0 / (PPQN as f64 * (secs - s_prev))) as f32;
        if !(MIN_BPM..=MAX_BPM).contains(&bpm) {
            return None;
        }
        let after = self.tempo_at(target_tick);
        let mut points: Vec<TempoPoint> = vec![TempoPoint { tick: 0, bpm: self.bpm, ramp: false }];
        points.extend(self.tempo_map.iter().copied());
        for p in &mut points {
            if p.tick == t_prev {
                p.bpm = bpm;
                p.ramp = false;
            }
        }
        if !points.iter().any(|p| p.tick == target_tick) {
            points.push(TempoPoint { tick: target_tick, bpm: after, ramp: false });
        }
        let _ = b_prev;
        Some(points)
    }

    /// Seconds from tick zero to `tick`.
    pub fn tick_to_seconds(&self, tick: u64) -> f64 {
        let mut secs = 0.0f64;
        let (mut t0, mut b0, mut ramp) = (0u64, self.bpm, false);
        for p in &self.tempo_map {
            let t1 = p.tick.min(tick);
            if t1 > t0 {
                secs += segment_seconds(t0, b0, if ramp { Some((p.tick, p.bpm)) } else { None }, t1);
            }
            if p.tick >= tick {
                return secs;
            }
            t0 = p.tick;
            b0 = p.bpm;
            ramp = p.ramp;
        }
        if tick > t0 {
            secs += segment_seconds(t0, b0, None, tick);
        }
        secs
    }

    /// The tick reached `secs` after tick zero.
    pub fn seconds_to_tick(&self, secs: f64) -> u64 {
        if secs <= 0.0 {
            return 0;
        }
        let mut left = secs;
        let (mut t0, mut b0, mut ramp) = (0u64, self.bpm, false);
        for p in &self.tempo_map {
            let seg = segment_seconds(t0, b0, if ramp { Some((p.tick, p.bpm)) } else { None }, p.tick);
            if left < seg {
                return t0 + ticks_into_segment(t0, b0, if ramp { Some((p.tick, p.bpm)) } else { None }, left);
            }
            left -= seg;
            t0 = p.tick;
            b0 = p.bpm;
            ramp = p.ramp;
        }
        t0 + (left * b0 as f64 * PPQN as f64 / 60.0).round() as u64
    }

    /// The meter in force at `tick`.
    pub fn meter_at(&self, tick: u64) -> (u8, u8) {
        let mut sig = self.time_sig;
        for c in &self.meter_map {
            if c.tick > tick {
                break;
            }
            sig = c.sig();
        }
        (sig.0.max(1), sig.1.max(1))
    }

    /// The bar `tick` falls in (zero-based) and where that bar starts.
    pub fn bar_start_of(&self, tick: u64) -> (u32, u64) {
        let (mut bar, mut seg_start, mut sig) = (0u64, 0u64, self.time_sig);
        for c in &self.meter_map {
            if c.tick > tick {
                break;
            }
            let bt = bar_ticks_of(sig);
            bar += (c.tick - seg_start) / bt;
            seg_start = c.tick;
            sig = c.sig();
        }
        let bt = bar_ticks_of(sig);
        let n = (tick - seg_start) / bt;
        ((bar + n) as u32, seg_start + n * bt)
    }

    /// The first tick of bar `bar` (zero-based).
    pub fn tick_of_bar(&self, bar: u32) -> u64 {
        let want = bar as u64;
        let (mut bar_at, mut seg_start, mut sig) = (0u64, 0u64, self.time_sig);
        for c in &self.meter_map {
            let bt = bar_ticks_of(sig);
            let bars_in_seg = (c.tick - seg_start) / bt;
            if bar_at + bars_in_seg > want {
                break;
            }
            bar_at += bars_in_seg;
            seg_start = c.tick;
            sig = c.sig();
        }
        seg_start + (want - bar_at) * bar_ticks_of(sig)
    }

    /// Bar, beat (both from one) and the tick into the beat, at `tick`.
    pub fn bar_beat_of(&self, tick: u64) -> (u32, u32, u32) {
        let (bar, start) = self.bar_start_of(tick);
        let sig = self.meter_at(tick);
        let tpbeat = (PPQN as u64 * 4 / sig.1 as u64).max(1);
        let rem = tick - start;
        (bar + 1, (rem / tpbeat) as u32 + 1, (rem % tpbeat) as u32)
    }

    /// Every bar starting in `[from, to)`: (bar index from zero, start tick,
    /// meter). Bounded, so a ruler zoomed out over an empty session ends.
    pub fn bars_between(&self, from: u64, to: u64, cap: usize) -> Vec<(u32, u64, (u8, u8))> {
        let mut out = Vec::new();
        let (mut bar, mut start) = self.bar_start_of(from);
        while start < to && out.len() < cap {
            let sig = self.meter_at(start);
            out.push((bar, start, sig));
            start += bar_ticks_of(sig);
            bar += 1;
        }
        out
    }

    /// `tick` on the grid of its own bar: `div` steps per bar, one meaning the
    /// bar line, zero meaning no grid.
    pub fn snap_tick(&self, tick: u64, div: u32) -> u64 {
        if div == 0 {
            return tick;
        }
        let (_, start) = self.bar_start_of(tick);
        let step = (bar_ticks_of(self.meter_at(tick)) / div as u64).max(1);
        start + ((tick - start + step / 2) / step) * step
    }

    /// The tempo map and the meter map, carried with the rest of the time
    /// line when a range moves.
    pub(crate) fn shift_time_maps(&mut self, from: u64, span: u64, delta: i64) {
        let end = from + span;
        let mv = |t: u64| -> u64 {
            if t >= from && t < end { (t as i64 + delta).max(0) as u64 } else { t }
        };
        for p in &mut self.tempo_map {
            p.tick = mv(p.tick);
        }
        let points = std::mem::take(&mut self.tempo_map);
        self.set_tempo_map(points);
        for c in &mut self.meter_map {
            c.tick = mv(c.tick);
        }
        let changes = std::mem::take(&mut self.meter_map);
        self.set_meter_map(changes);
    }

    pub(crate) fn remove_time_maps(&mut self, from: u64, span: u64) {
        let end = from + span;
        self.tempo_map.retain(|p| !(p.tick >= from && p.tick < end));
        for p in &mut self.tempo_map {
            if p.tick >= end { p.tick -= span; }
        }
        self.meter_map.retain(|c| !(c.tick >= from && c.tick < end));
        for c in &mut self.meter_map {
            if c.tick >= end { c.tick -= span; }
        }
        let changes = std::mem::take(&mut self.meter_map);
        self.set_meter_map(changes);
    }
}

/// Seconds over `[t0, t1]` at `b0`, or ramping to `(tick, bpm)` if given.
fn segment_seconds(t0: u64, b0: f32, ramp_to: Option<(u64, f32)>, t1: u64) -> f64 {
    let span = (t1 - t0) as f64;
    let per_beat = 60.0 / PPQN as f64;
    match ramp_to {
        Some((te, b1)) if te > t0 && (b1 - b0).abs() > 1e-6 => {
            // bpm(t) = b0 + (b1 - b0) (t - t0) / (te - t0); integrate 1/bpm.
            let full = (te - t0) as f64;
            let b_at = b0 as f64 + (b1 - b0) as f64 * span / full;
            per_beat * full / (b1 - b0) as f64 * (b_at / b0 as f64).ln()
        }
        _ => per_beat * span / b0 as f64,
    }
}

/// Ticks past `t0` reached after `secs`, on the same segment law.
fn ticks_into_segment(t0: u64, b0: f32, ramp_to: Option<(u64, f32)>, secs: f64) -> u64 {
    let per_beat = 60.0 / PPQN as f64;
    match ramp_to {
        Some((te, b1)) if te > t0 && (b1 - b0).abs() > 1e-6 => {
            let full = (te - t0) as f64;
            let k = (b1 - b0) as f64 / full;
            let b_at = b0 as f64 * (secs * k / per_beat).exp();
            ((b_at - b0 as f64) / k).round().max(0.0) as u64
        }
        _ => (secs * b0 as f64 / per_beat).round() as u64,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_landing_holds_the_moment_and_the_tempo_after() {
        let mut s = crate::data::Session::default();
        s.bpm = 120.0;
        let target = 4 * 4 * PPQN as u64;
        let map = s.tempo_map_landing(10.0, target).unwrap();
        s.set_tempo_map(map);
        assert!((s.bpm - 96.0).abs() < 1e-3, "{}", s.bpm);
        assert!((s.tick_to_seconds(target) - 10.0).abs() < 1e-6);
        assert!((s.tempo_at(target + 1) - 120.0).abs() < 1e-3);
        assert!(s.tempo_map_landing(0.0, target).is_none());
    }

    use super::*;

    fn bar(n: u64) -> u64 { n * 4 * PPQN as u64 }

    #[test]
    fn a_held_point_holds_and_a_ramp_ramps() {
        let mut s = Session::default();
        s.bpm = 120.0;
        s.set_tempo_map(vec![
            TempoPoint { tick: bar(4), bpm: 60.0, ramp: true },
            TempoPoint { tick: bar(8), bpm: 120.0, ramp: false },
        ]);
        assert_eq!(s.tempo_at(bar(2)), 120.0);
        assert_eq!(s.tempo_at(bar(4)), 60.0);
        assert!((s.tempo_at(bar(6)) - 90.0).abs() < 1e-4);
        assert_eq!(s.tempo_at(bar(9)), 120.0);
    }

    #[test]
    fn seconds_round_trip_through_a_ramp() {
        let mut s = Session::default();
        s.bpm = 120.0;
        s.set_tempo_map(vec![
            TempoPoint { tick: bar(2), bpm: 60.0, ramp: true },
            TempoPoint { tick: bar(4), bpm: 180.0, ramp: false },
        ]);
        // Two bars of 4/4 at 120 are four seconds.
        assert!((s.tick_to_seconds(bar(2)) - 4.0).abs() < 1e-9);
        for t in [0u64, 100, bar(1), bar(2) + 7, bar(3), bar(4) - 1, bar(5), bar(12)] {
            let back = s.seconds_to_tick(s.tick_to_seconds(t));
            assert!((back as i64 - t as i64).abs() <= 1, "tick {t} came back as {back}");
        }
        // A ramp from 60 to 180 over two bars takes less time than holding 60.
        let ramp = s.tick_to_seconds(bar(4)) - s.tick_to_seconds(bar(2));
        assert!(ramp < 8.0 && ramp > 8.0 / 3.0, "{ramp}");
    }

    #[test]
    fn meter_changes_move_the_bar_lines() {
        let mut s = Session::default();
        s.time_sig = (4, 4);
        s.set_meter_map(vec![MeterChange { tick: bar(2), num: 3, den: 4 }]);
        assert_eq!(s.meter_at(bar(2) - 1), (4, 4));
        assert_eq!(s.meter_at(bar(2)), (3, 4));
        let three = 3 * PPQN as u64;
        assert_eq!(s.tick_of_bar(2), bar(2));
        assert_eq!(s.tick_of_bar(3), bar(2) + three);
        assert_eq!(s.bar_start_of(bar(2) + three + 5), (3, bar(2) + three));
        assert_eq!(s.bar_beat_of(bar(2) + three + PPQN as u64 + 3), (4, 2, 3));
        let bars = s.bars_between(0, bar(2) + 2 * three, 100);
        assert_eq!(bars.len(), 4);
        assert_eq!(bars[3], (3, bar(2) + three, (3, 4)));
    }

    #[test]
    fn a_change_off_the_bar_line_is_put_on_it() {
        let mut s = Session::default();
        s.set_meter_map(vec![MeterChange { tick: bar(1) + 17, num: 7, den: 8 }]);
        assert_eq!(s.meter_map[0].tick, bar(1));
        s.set_meter_map(vec![MeterChange { tick: 0, num: 6, den: 8 }]);
        assert!(s.meter_map.is_empty());
        assert_eq!(s.time_sig, (6, 8));
    }

    #[test]
    fn snapping_follows_the_local_meter() {
        let mut s = Session::default();
        s.set_meter_map(vec![MeterChange { tick: bar(1), num: 3, den: 4 }]);
        let beat = PPQN as u64;
        assert_eq!(s.snap_tick(bar(1) + 2 * beat + 100, 3), bar(1) + 2 * beat);
        assert_eq!(s.snap_tick(bar(1) + 3 * beat - 10, 1), bar(1) + 3 * beat);
        assert_eq!(s.snap_tick(bar(1) + 3 * beat - 10, 0), bar(1) + 3 * beat - 10);
    }

    #[test]
    fn the_maps_ripple_with_the_time_line() {
        let mut s = Session::default();
        s.set_tempo_map(vec![TempoPoint { tick: bar(3), bpm: 90.0, ramp: false }]);
        s.set_meter_map(vec![MeterChange { tick: bar(3), num: 3, den: 4 }]);
        s.shift_range(bar(2), bar(4), bar(1) as i64);
        assert_eq!(s.tempo_map[0].tick, bar(4));
        assert_eq!(s.meter_map[0].tick, bar(4));
        s.remove_range(bar(1), bar(1));
        assert_eq!(s.tempo_map[0].tick, bar(3));
        assert_eq!(s.meter_map[0].tick, bar(3));
        s.remove_range(bar(3), bar(1));
        assert!(s.tempo_map.is_empty() && s.meter_map.is_empty());
    }

    #[test]
    fn markers_follow_the_time_edits() {
        use crate::data::Marker;
        let mut s = Session::default();
        s.set_markers(vec![
            Marker { tick: 3000, name: "b".into() },
            Marker { tick: 1000, name: "a".into() },
        ]);
        assert_eq!(s.markers[0].name, "a", "markers are kept sorted");
        assert_eq!(s.marker_before(3000).map(|m| m.tick), Some(1000));
        assert_eq!(s.marker_after(1000).map(|m| m.tick), Some(3000));
        assert!(s.marker_after(3000).is_none());
        s.shift_range(2000, 2000, 500);
        assert_eq!(s.markers[1].tick, 3500);
        s.remove_range(0, 2000);
        assert_eq!(s.markers.len(), 1);
        assert_eq!(s.markers[0].tick, 1500);
    }
}
