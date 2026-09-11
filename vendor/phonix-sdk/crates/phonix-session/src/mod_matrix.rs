//! Global modulation matrix.
//!
//! A session-level graph of `ModSourceDef`s (LFOs, envelopes, macros, random,
//! MIDI CC) and `ModRoute`s that connect each source to a `ModTarget`
//! (mixer volume/pan/sends, track/master FX params, engine params). The
//! audio thread evaluates every source once per block, then fans out the
//! modulation deltas to their targets. The user-set "base" value of each
//! target is never mutated; modulated values are computed as
//! `effective = base + Σ (curve(source) * amount * range)`.
//!
//! Allocation-free in steady state: all runtime scratch (source values,
//! envelope state, LFO phase) is preallocated and updated in place.

use serde::{Serialize, Deserialize};

// ── Sources ──────────────────────────────────────────────────────────────────

/// Waveform shape for an LFO source.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfoShape {
    Sine,
    Triangle,
    Saw,      // rising ramp, −1..1
    Square,
    SampleHold,
    SmoothRandom,
}

impl Default for LfoShape { fn default() -> Self { LfoShape::Sine } }

/// How an LFO tracks time. Free runs in Hz; Sync locks phase to the transport.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LfoSync {
    Free,
    /// Tempo-synced. `division` = number of LFO cycles per bar.
    /// 1 → 1 cycle / bar, 4 → quarter note, 16 → sixteenth, 0.5 → 2 bars.
    /// Stored as a small enum for clarity + restore safety.
    BarsPerCycle(LfoDivision),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum LfoDivision {
    D4Bars, D2Bars, D1Bar, D1_2, D1_4, D1_8, D1_16, D1_32,
}

impl LfoDivision {
    /// Cycles per bar (the LFO completes this many cycles over one bar).
    pub fn cycles_per_bar(self) -> f32 {
        match self {
            Self::D4Bars => 0.25,
            Self::D2Bars => 0.5,
            Self::D1Bar  => 1.0,
            Self::D1_2   => 2.0,
            Self::D1_4   => 4.0,
            Self::D1_8   => 8.0,
            Self::D1_16  => 16.0,
            Self::D1_32  => 32.0,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::D4Bars => "4 bars",
            Self::D2Bars => "2 bars",
            Self::D1Bar  => "1 bar",
            Self::D1_2   => "1/2",
            Self::D1_4   => "1/4",
            Self::D1_8   => "1/8",
            Self::D1_16  => "1/16",
            Self::D1_32  => "1/32",
        }
    }
    pub const ALL: &'static [LfoDivision] = &[
        Self::D4Bars, Self::D2Bars, Self::D1Bar,
        Self::D1_2, Self::D1_4, Self::D1_8, Self::D1_16, Self::D1_32,
    ];
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvTrigger {
    /// Triggered once when the transport starts playing.
    OnPlay,
    /// Retriggered on every bar boundary.
    EachBar,
    /// Retriggered on every beat.
    EachBeat,
}

/// Envelope state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum EnvStage {
    #[default] Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// A named modulation source.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModSourceDef {
    pub name:    String,
    pub kind:    ModSourceKind,
    /// Non-serialized runtime state, reset on load.
    #[serde(skip)]
    pub runtime: ModSourceRuntime,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ModSourceKind {
    Lfo {
        shape: LfoShape,
        rate_hz: f32,      // used when sync = Free
        sync: LfoSync,
        /// Initial phase in [0, 1)
        phase0: f32,
        /// If true, output is 0..1; otherwise -1..1
        unipolar: bool,
    },
    Env {
        attack_ms: f32,
        decay_ms:  f32,
        sustain:   f32,
        release_ms: f32,
        trigger:   EnvTrigger,
    },
    Macro {
        value: f32,          // user-set 0..1
        smoothing_ms: f32,
    },
    Random {
        rate_hz: f32,
        smooth: bool,        // true = SmoothRandom, false = S&H
    },
    MidiCc {
        channel: u8,         // 0..15, 0xFF = any
        cc: u8,              // 0..127
    },
    /// The loudness of a track, followed: its block RMS times `gain`,
    /// capped at one, smoothed with an attack and a release.
    Follower {
        track: u16,
        attack_ms: f32,
        release_ms: f32,
        gain: f32,
    },
}

impl ModSourceKind {
    pub fn type_label(&self) -> &'static str {
        match self {
            Self::Lfo {..}   => "LFO",
            Self::Env {..}   => "Env",
            Self::Macro {..} => "Macro",
            Self::Random {..}=> "Random",
            Self::MidiCc {..}=> "CC",
            Self::Follower {..} => "Follow",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModSourceRuntime {
    pub phase:       f32,      // LFO / Random phase accumulator, [0, 1)
    pub value:       f32,      // current output
    pub env_stage:   EnvStage,
    pub env_time_s:  f32,      // seconds in current stage
    pub env_level:   f32,      // current envelope level
    pub smoothed:    f32,      // one-pole smoother state (Macro, CC)
}

// ── Targets ──────────────────────────────────────────────────────────────────

/// Address of any modulatable parameter. Kept as a small enum so routes
/// can be compared for equality and grouped cheaply.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModTarget {
    /// Channel strip volume. Base 0..1, mod clamped into same range.
    MixerVolume { track: u16 },
    /// Channel strip pan. Base −1..1.
    MixerPan    { track: u16 },
    /// Send bus level. Base 0..1.
    MixerSend   { track: u16, send: u8 },
    /// Track insert FX param (slot 0..4, pid 0..99).
    TrackFx     { track: u16, slot: u8, pid: u8 },
    /// Master insert FX param (slot 0..8, pid 0..99).
    MasterFx    { slot: u8, pid: u8 },
    /// Per-engine native parameter (filter cutoff, resonance, etc.).
    Engine      { track: u16, pid: u16 },
    /// Morph position between macro snapshots (0..1).
    MorphPosition,
    /// Modulate a source's primary parameter (LFO rate, Macro value, Random
    /// rate). Enables mod-to-mod routing: e.g. an envelope controlling an
    /// LFO's speed, or an LFO sweeping a macro.
    SourceParam { source_idx: u16, param: u8 },
}

impl ModTarget {
    /// Per-target display range, used to scale the route amount. Routes
    /// store a unit amount in −1..1; this converts to the real parameter's
    /// natural range before adding to the base value.
    pub fn range(self) -> f32 {
        match self {
            Self::MixerVolume { .. }  => 1.0,
            Self::MixerPan    { .. }  => 2.0,
            Self::MixerSend   { .. }  => 1.0,
            Self::TrackFx     { .. }  => 1.0,
            Self::MasterFx    { .. }  => 1.0,
            Self::Engine      { .. }  => 1.0,
            Self::MorphPosition      => 1.0,
            Self::SourceParam { .. } => 1.0,
        }
    }

    /// Short, stable label used in the GUI target dropdown.
    pub fn label(self) -> String {
        match self {
            Self::MixerVolume { track }        => format!("Trk {}: Volume", track + 1),
            Self::MixerPan    { track }        => format!("Trk {}: Pan",    track + 1),
            Self::MixerSend   { track, send }  => format!("Trk {}: Send {}", track + 1, send + 1),
            Self::TrackFx     { track, slot, pid } =>
                format!("Trk {}: FX{} p{}", track + 1, slot + 1, pid),
            Self::MasterFx    { slot, pid }    => format!("Master FX{} p{}", slot + 1, pid),
            Self::Engine      { track, pid }   => {
                format!("Trk {}: Engine p{}", track + 1, pid)
            }
            Self::MorphPosition => "Morph Position".to_string(),
            Self::SourceParam { source_idx, param } => {
                let p = match param { 0 => "Rate/Value", _ => "Param" };
                format!("Src {}: {}", source_idx + 1, p)
            }
        }
    }
}

/// Curve shape applied to a route's source value before scaling.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModCurve {
    Linear,
    Exp,     // v² preserving sign
    Log,     // sign(v) * sqrt(|v|)
    Invert,  // -v
}

impl Default for ModCurve { fn default() -> Self { ModCurve::Linear } }

impl ModCurve {
    #[inline]
    pub fn apply(self, v: f32) -> f32 {
        match self {
            Self::Linear => v,
            Self::Exp    => v * v * v.signum(),
            Self::Log    => v.signum() * v.abs().sqrt(),
            Self::Invert => -v,
        }
    }
}

/// A single wire: source → target with amount and curve.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModRoute {
    pub source_idx: u16,
    pub target:     ModTarget,
    pub amount:     f32,    // −1..1
    pub curve:      ModCurve,
    pub enabled:    bool,
}

impl ModRoute {
    pub fn new(source_idx: u16, target: ModTarget, amount: f32) -> Self {
        Self { source_idx, target, amount, curve: ModCurve::Linear, enabled: true }
    }
}

// ── Matrix ───────────────────────────────────────────────────────────────────

/// A frozen capture of all Macro source values at a point in time.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MacroSnapshot {
    pub name:   String,
    /// (source_idx, value) pairs — only Macro sources are stored.
    pub values: Vec<(u16, f32)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ModMatrix {
    pub sources: Vec<ModSourceDef>,
    pub routes:  Vec<ModRoute>,
    /// Up to 8 macro snapshots for scene morphing.
    #[serde(default)]
    pub snapshots: Vec<MacroSnapshot>,
    /// Morph position 0..1. Interpolates between adjacent snapshots.
    /// 0.0 = snapshot 0, 1.0 = last snapshot. The morph slider drives this.
    #[serde(default)]
    pub morph_position: f32,
}

impl ModMatrix {
    pub fn add_source(&mut self, def: ModSourceDef) -> u16 {
        let idx = self.sources.len() as u16;
        self.sources.push(def);
        idx
    }

    pub fn remove_source(&mut self, idx: u16) {
        if (idx as usize) >= self.sources.len() { return; }
        self.sources.remove(idx as usize);
        // Remove routes pointing at this source, and decrement indices above.
        self.routes.retain(|r| r.source_idx != idx);
        for r in &mut self.routes {
            if r.source_idx > idx { r.source_idx -= 1; }
        }
    }

    pub fn add_route(&mut self, route: ModRoute) {
        // Ignore duplicates (same source × same target).
        if self.routes.iter().any(|r| r.source_idx == route.source_idx && r.target == route.target) {
            return;
        }
        self.routes.push(route);
    }

    pub fn remove_route(&mut self, idx: usize) {
        if idx < self.routes.len() { self.routes.remove(idx); }
    }

    /// Routes that touch this target (enabled only).
    pub fn routes_for(&self, target: ModTarget) -> impl Iterator<Item = &ModRoute> {
        self.routes.iter().filter(move |r| r.enabled && r.target == target)
    }

    /// Count of routes on the given target (used by the knob overlay to decide
    /// whether to draw the modulation arc at all).
    pub fn route_count_on(&self, target: ModTarget) -> usize {
        self.routes.iter().filter(|r| r.enabled && r.target == target).count()
    }

    /// Store a snapshot of all current Macro source values.
    pub fn store_snapshot(&mut self, name: String) {
        let values: Vec<(u16, f32)> = self.sources.iter().enumerate()
            .filter_map(|(i, s)| {
                if let ModSourceKind::Macro { value, .. } = s.kind {
                    Some((i as u16, value))
                } else {
                    None
                }
            })
            .collect();
        if self.snapshots.len() >= 8 { return; }
        self.snapshots.push(MacroSnapshot { name, values });
    }

    /// Delete a snapshot by index.
    pub fn delete_snapshot(&mut self, idx: usize) {
        if idx < self.snapshots.len() { self.snapshots.remove(idx); }
    }

    /// Recall a snapshot — set all macros to the snapshot's stored values.
    pub fn recall_snapshot(&mut self, idx: usize) {
        if let Some(snap) = self.snapshots.get(idx) {
            for &(src_idx, val) in &snap.values {
                if let Some(src) = self.sources.get_mut(src_idx as usize) {
                    if let ModSourceKind::Macro { ref mut value, .. } = src.kind {
                        *value = val;
                    }
                }
            }
        }
    }

    /// Apply morph interpolation: blend macro values between two adjacent
    /// snapshots based on `morph_position`. Called each audio block when
    /// there are >= 2 snapshots.
    pub fn apply_morph(&mut self) {
        let n = self.snapshots.len();
        if n < 2 { return; }

        // Map morph_position 0..1 to a fractional index in [0, n-1].
        let pos = self.morph_position.clamp(0.0, 1.0) * (n - 1) as f32;
        let lo = (pos.floor() as usize).min(n - 2);
        let hi = lo + 1;
        let frac = pos - lo as f32;

        let snap_lo = &self.snapshots[lo];
        let snap_hi = &self.snapshots[hi];

        // For each macro source, interpolate between the two snapshots.
        for &(src_idx, val_lo) in &snap_lo.values {
            let val_hi = snap_hi.values.iter()
                .find(|(i, _)| *i == src_idx)
                .map(|(_, v)| *v)
                .unwrap_or(val_lo);
            let blended = val_lo + (val_hi - val_lo) * frac;
            if let Some(src) = self.sources.get_mut(src_idx as usize) {
                if let ModSourceKind::Macro { ref mut value, .. } = src.kind {
                    *value = blended;
                }
            }
        }
    }

    /// Evaluate every source once per audio block, writing their outputs
    /// into `values` (resized to match source count). Allocation-free in
    /// steady state — `values` is passed in from a pre-sized scratch buffer.
    pub fn evaluate_all(&mut self, values: &mut Vec<f32>, ctx: &SourceContext<'_>) {
        values.resize(self.sources.len(), 0.0);
        for (i, src) in self.sources.iter_mut().enumerate() {
            values[i] = evaluate_source(&src.kind, &mut src.runtime, ctx);
        }
    }

    /// Compute the current modulated value for a target (base + Σ deltas).
    /// Returns None if no enabled route touches this target. Used by the
    /// GUI overlay to draw the live arc — never called on the audio thread.
    pub fn modulated_value(&self, values: &[f32], target: ModTarget, base: f32) -> Option<f32> {
        let mut delta = 0.0;
        let mut any = false;
        for r in &self.routes {
            if !r.enabled || r.target != target { continue; }
            if let Some(v) = values.get(r.source_idx as usize) {
                delta += r.curve.apply(*v) * r.amount * r.target.range();
                any = true;
            }
        }
        if any { Some(base + delta) } else { None }
    }
}

// ── Runtime evaluation helpers ───────────────────────────────────────────────

/// Small context passed to source evaluation each block.
pub struct SourceContext<'a> {
    /// RMS of every track over the last block, by track.
    pub track_levels: &'a [f32],
    pub sr:           f32,  // sample rate
    pub frames:       u32,  // frames this block
    pub bpm:          f32,
    pub transport_tick: u64,  // absolute PPQN tick at block start
    pub ppqn:         u32,
    pub time_sig_num: u8,
    pub time_sig_den: u8,
    pub playing:      bool,
    pub bar_index:    u64,   // absolute bar number
    pub beat_index:   u64,   // absolute beat number
    pub bar_phase:    f32,   // 0..1 within the current bar at block start
    /// True on the first block after transport.playing flipped from false to
    /// true. Used to retrigger OnPlay envelopes.
    pub just_started: bool,
    /// True if a bar boundary fell between the previous block's start and
    /// this block's start. Used to retrigger EachBar envelopes.
    pub bar_crossed:  bool,
    /// True if a beat boundary fell between the previous block's start and
    /// this block's start. Used to retrigger EachBeat envelopes.
    pub beat_crossed: bool,
}

#[inline]
fn lfo_sample(shape: LfoShape, phase: f32, rt_phase: &mut f32) -> f32 {
    // phase in [0,1)
    match shape {
        LfoShape::Sine     => (phase * std::f32::consts::TAU).sin(),
        LfoShape::Triangle => {
            let t = phase * 4.0;
            if t < 1.0 { t }
            else if t < 3.0 { 2.0 - t }
            else { t - 4.0 }
        }
        LfoShape::Saw      => 2.0 * phase - 1.0,
        LfoShape::Square   => if phase < 0.5 { 1.0 } else { -1.0 },
        LfoShape::SampleHold => {
            // rt_phase abused as held value storage via fractional trick:
            // caller updates rt_phase when a new cycle begins (done in eval()).
            *rt_phase
        }
        LfoShape::SmoothRandom => *rt_phase, // same
    }
}

/// Evaluate a single source for the current block. Returns its output in
/// the natural range (either −1..1 or 0..1 depending on source kind /
/// unipolar flag). Mutates `runtime` in place.
pub fn evaluate_source(
    kind:    &ModSourceKind,
    runtime: &mut ModSourceRuntime,
    ctx:     &SourceContext,
) -> f32 {
    match *kind {
        ModSourceKind::Lfo { shape, rate_hz, sync, phase0, unipolar } => {
            // Compute the phase at block start.
            let cycles_per_sec = match sync {
                LfoSync::Free => rate_hz.max(0.0001),
                LfoSync::BarsPerCycle(div) => {
                    // cycles_per_bar * bars_per_sec
                    let secs_per_bar = 60.0 * ctx.time_sig_num as f32 * 4.0
                                     / (ctx.bpm * ctx.time_sig_den as f32);
                    div.cycles_per_bar() / secs_per_bar.max(1e-6)
                }
            };
            let advance = cycles_per_sec * (ctx.frames as f32 / ctx.sr);
            runtime.phase += advance;
            while runtime.phase >= 1.0 { runtime.phase -= 1.0; }
            while runtime.phase < 0.0 { runtime.phase += 1.0; }
            let p = (runtime.phase + phase0).fract();

            // For S&H / SmoothRandom, re-roll on cycle wraps.
            if matches!(shape, LfoShape::SampleHold | LfoShape::SmoothRandom) {
                // Advance tracker: if phase wrapped this block, roll a new target.
                // Store current held value in runtime.value (written outside).
                // Crude but fine: re-roll whenever phase < advance (just wrapped).
                if runtime.phase < advance.fract() {
                    let r = fast_rand(ctx.transport_tick ^ (cycles_per_sec.to_bits() as u64));
                    runtime.value = r * 2.0 - 1.0;
                }
                if matches!(shape, LfoShape::SmoothRandom) {
                    // One-pole toward runtime.value
                    let a = 0.1;
                    runtime.smoothed += (runtime.value - runtime.smoothed) * a;
                    let out = runtime.smoothed;
                    if unipolar { 0.5 * (out + 1.0) } else { out }
                } else {
                    let out = runtime.value;
                    if unipolar { 0.5 * (out + 1.0) } else { out }
                }
            } else {
                let mut held = 0.0;
                let out = lfo_sample(shape, p, &mut held);
                if unipolar { 0.5 * (out + 1.0) } else { out }
            }
        }
        ModSourceKind::Env { attack_ms, decay_ms, sustain, release_ms, trigger } => {
            // Trigger logic — on rising edges of the chosen event.
            let retrigger = if !ctx.playing {
                false
            } else {
                match trigger {
                    EnvTrigger::OnPlay  => ctx.just_started,
                    EnvTrigger::EachBar => ctx.just_started || ctx.bar_crossed,
                    EnvTrigger::EachBeat=> ctx.just_started || ctx.beat_crossed,
                }
            };
            if retrigger {
                runtime.env_stage = EnvStage::Attack;
                runtime.env_time_s = 0.0;
                runtime.env_level  = 0.0;
            }
            // When the transport stops, release the envelope so it fades out.
            if !ctx.playing && !matches!(runtime.env_stage, EnvStage::Idle | EnvStage::Release) {
                runtime.env_stage = EnvStage::Release;
                runtime.env_time_s = 0.0;
            }

            let dt = ctx.frames as f32 / ctx.sr;
            match runtime.env_stage {
                EnvStage::Idle => runtime.env_level = 0.0,
                EnvStage::Attack => {
                    let t = (attack_ms * 1e-3).max(1e-4);
                    runtime.env_level = (runtime.env_time_s / t).min(1.0);
                    runtime.env_time_s += dt;
                    if runtime.env_time_s >= t { runtime.env_stage = EnvStage::Decay; runtime.env_time_s = 0.0; }
                }
                EnvStage::Decay => {
                    let t = (decay_ms * 1e-3).max(1e-4);
                    let frac = (runtime.env_time_s / t).min(1.0);
                    runtime.env_level = 1.0 - frac * (1.0 - sustain);
                    runtime.env_time_s += dt;
                    if runtime.env_time_s >= t { runtime.env_stage = EnvStage::Sustain; runtime.env_time_s = 0.0; }
                }
                EnvStage::Sustain => runtime.env_level = sustain,
                EnvStage::Release => {
                    let t = (release_ms * 1e-3).max(1e-4);
                    let frac = (runtime.env_time_s / t).min(1.0);
                    runtime.env_level = sustain * (1.0 - frac);
                    runtime.env_time_s += dt;
                    if runtime.env_time_s >= t { runtime.env_stage = EnvStage::Idle; runtime.env_level = 0.0; }
                }
            }
            runtime.env_level
        }
        ModSourceKind::Macro { value, smoothing_ms } => {
            let a = if smoothing_ms <= 0.0 {
                1.0
            } else {
                let tau_s = smoothing_ms * 1e-3;
                1.0 - (-1.0 / (tau_s * ctx.sr / ctx.frames as f32)).exp()
            };
            runtime.smoothed += (value - runtime.smoothed) * a.clamp(0.0, 1.0);
            runtime.smoothed
        }
        ModSourceKind::Random { rate_hz, smooth } => {
            let cps = rate_hz.max(0.01);
            let advance = cps * (ctx.frames as f32 / ctx.sr);
            runtime.phase += advance;
            if runtime.phase >= 1.0 {
                runtime.phase -= runtime.phase.floor();
                let r = fast_rand(ctx.transport_tick.wrapping_add(cps.to_bits() as u64));
                runtime.value = r * 2.0 - 1.0;
            }
            if smooth {
                let a = 0.1;
                runtime.smoothed += (runtime.value - runtime.smoothed) * a;
                runtime.smoothed
            } else {
                runtime.value
            }
        }
        ModSourceKind::MidiCc { .. } => {
            // runtime.value is written externally by the MIDI handler.
            runtime.value
        }
        ModSourceKind::Follower { track, attack_ms, release_ms, gain } => {
            let level = (ctx.track_levels.get(track as usize).copied().unwrap_or(0.0) * gain).clamp(0.0, 1.0);
            let dt = ctx.frames as f32 / ctx.sr.max(1.0);
            let tau = if level > runtime.value { attack_ms } else { release_ms } * 1e-3;
            let a = if tau <= 0.0 { 1.0 } else { 1.0 - (-dt / tau).exp() };
            runtime.value += (level - runtime.value) * a;
            runtime.value
        }
    }
}

#[inline]
fn fast_rand(seed: u64) -> f32 {
    // Xorshift to produce 0..1
    let mut x = seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(0xDEADBEEFCAFEBABE);
    x ^= x >> 33; x = x.wrapping_mul(0xFF51AFD7ED558CCD);
    x ^= x >> 33; x = x.wrapping_mul(0xC4CEB9FE1A85EC53);
    x ^= x >> 33;
    (x as u32 as f32) / (u32::MAX as f32)
}


#[cfg(test)]
mod follower_tests {
    use super::*;

    fn ctx(levels: &[f32]) -> SourceContext<'_> {
        SourceContext {
            track_levels: levels, sr: 48_000.0, frames: 480, bpm: 120.0, transport_tick: 0, ppqn: 960,
            time_sig_num: 4, time_sig_den: 4, playing: true, bar_index: 0, beat_index: 0, bar_phase: 0.0,
            just_started: false, bar_crossed: false, beat_crossed: false,
        }
    }

    #[test]
    fn a_follower_rises_with_its_attack_and_falls_with_its_release() {
        let kind = ModSourceKind::Follower { track: 1, attack_ms: 10.0, release_ms: 100.0, gain: 2.0 };
        let mut rt = ModSourceRuntime::default();
        let loud = [0.0f32, 0.25];
        let mut v = 0.0;
        for _ in 0..10 { v = evaluate_source(&kind, &mut rt, &ctx(&loud)); }
        assert!(v > 0.49, "after 100 ms of a 10 ms attack: {v}");
        let quiet = [0.0f32, 0.0];
        let one = evaluate_source(&kind, &mut rt, &ctx(&quiet));
        assert!(one < v && one > 0.4, "10 ms into a 100 ms release: {one}");
        assert_eq!(evaluate_source(&kind, &mut ModSourceRuntime::default(), &ctx(&[0.9])), 0.0, "a track past the list is silent");
    }
}
