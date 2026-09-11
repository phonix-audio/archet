//! The session model: Session, Track, Pattern, the mixer, the chord track.

use serde::{Deserialize, Serialize};
use phonix_fx::{ChainSpec, SlotSpec};

pub use phonix_music::midi::{bar_ticks_of, MidiNote, NOTE_NAMES, PPQN, WHOLE_NOTE_TICKS};



/// What plays a track. Named by identity, never by an instrument's name: a
/// class id resolves the same way whether the host links the instrument or
/// scans for it, and a session written on one machine opens on another.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TrackSource {
    /// No instrument assigned yet.
    Empty,
    /// A VST3, hosted or built in. The class id is the identity; the path is
    /// where it was last found, and may be stale.
    Vst3 { class_id: String, plugin_path: Option<String> },
    Clap { plugin_id: String, plugin_path: Option<String> },
    AudioClip { path: String },
}

impl Default for TrackSource {
    fn default() -> Self { TrackSource::Empty }
}

// ── Track ─────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum TrackKind {
    Melodic,   // uses piano roll; notes have any pitch
    Drum,      // uses step grid; rows map to fixed pitches
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TrackConfig {
    pub name:          String,
    pub kind:          TrackKind,
    pub engine:        TrackSource,
    /// Arrangement: list of clip placements on the timeline.
    pub timeline:      Vec<ClipPlacement>,
    /// Session clips: one optional pattern per session slot (up to 8 slots).
    pub session_clips: Vec<Option<usize>>,
    /// Mixer
    pub volume:        f32,   // 0..1
    pub pan:           f32,   // -1..1
    pub mute:          bool,
    pub solo:          bool,
    #[serde(default = "default_sends")]
    pub sends:         Vec<f32>,
    pub color:         [u8; 3],  // RGB track color
    /// Per-track automation lanes (parameter name → automation points)
    #[serde(default)]
    pub automations:   Vec<AutomationLane>,
    /// The insert chain.
    #[serde(default)]
    pub fx:            ChainSpec,
    /// The insert chain as written before names were the wire format;
    /// `Session::take_legacy_fx` moves it into `fx` and clears it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fx_slots:      Vec<phonix_legacy::FxSlotState>,
    /// Per-track MIDI FX chain (up to 4 slots). Empty = no MIDI FX.
    #[serde(default)]
    pub midi_fx_slots: Vec<MidiFxSlotState>,
    /// Opaque VST3 plugin state blob (only used when engine == Vst3).
    #[serde(default)]
    pub vst3_state:    Vec<u8>,
    /// VST3 insert EFFECTS on this track's insert pipeline. Each entry
    /// records the plugin path, its opaque state blob and its `position`
    /// in the track's segment list (the built-in EffectsChain occupies one
    /// segment, so position 0 = before the built-in FX, 1+ = after).
    #[serde(default)]
    pub vst3_inserts:  Vec<Vst3InsertState>,
    /// Serialized built-in engine state (e.g. a patch as JSON). Restored on session load.
    #[serde(default)]
    pub engine_state:  Vec<u8>,
    /// When true, the engine's internal sequencer drives playback (Drum, TB-303).
    /// When false, PhonixSeq MIDI drives the engine (external mode).
    #[serde(default = "default_internal_seq")]
    pub internal_seq:  bool,
    /// Output routing: None = straight to the master/mix bus (default). Some(g) = route
    /// this track's output into group bus `g` instead, so a multi-FX submix (e.g. a
    /// hard-panned guitar double-track + chorus/delay/hall on the SUM) processes the
    /// summed stereo image, not each take post-pan (which collapses the FX width).
    #[serde(default)]
    pub output_group:  Option<u8>,
    /// Audio input routing: Some(src) = this track's audio is track `src`'s
    /// finished output (post output-stage + insert FX) COPIED in place of its
    /// own engine render (v1 replaces the engine signal — an FX-return /
    /// bus-processor track). None = normal engine render. Self- or
    /// out-of-range references are treated as None.
    #[serde(default)]
    pub audio_input:   Option<u16>,
    /// Scale lock: quantize this track's sequenced notes to the chord
    /// track's key/scale at the MIDI-FX chain entry. Off by default.
    #[serde(default)]
    pub scale_lock:    bool,
    /// Non-destructive FREEZE stash. When a synth track is frozen to audio, its
    /// original config (engine + patch state + MIDI clips + FX) is boxed here so
    /// `unfreeze_track` can restore it exactly, surviving save/reload (unlike the
    /// ephemeral undo stack). `None` for a normal track. Boxed to keep the enum
    /// non-recursive in size and skipped when empty in older sessions.
    #[serde(default)]
    pub frozen_original: Option<Box<TrackConfig>>,
    /// Automation along the arrangement, in absolute ticks: what the track
    /// reads while it plays and what it writes when armed. `automations`
    /// above is per pattern and loops with the clip.
    #[serde(default)]
    pub track_automation: Vec<AutomationLane>,
    /// How the track treats its arrangement automation.
    #[serde(default)]
    pub automation_mode: AutomationMode,
    /// A hardware or software MIDI destination the track's notes also go
    /// to, on top of its own instrument.
    #[serde(default)]
    pub midi_out: Option<MidiOutRoute>,
    /// The VCA fader scaling this track's volume.
    #[serde(default)]
    pub vca: Option<u8>,
    /// Crossfader side: 0 neither, 1 A, 2 B.
    #[serde(default)]
    pub xfade: u8,
    /// The folder the track sits in.
    #[serde(default)]
    pub folder: Option<u8>,
    /// Where the track sits when the mix is laid on more than two speakers.
    #[serde(default)]
    pub surround: crate::surround::SurroundPos,
    /// The recordings an audio track holds, oldest first.
    #[serde(default)]
    pub takes: Vec<Take>,
    /// Which take plays over which span: the comp, rendered into the
    /// track's clip. Segments are sorted and disjoint.
    #[serde(default)]
    pub comp: Vec<CompSegment>,
}

/// What a track does with its arrangement automation while the transport
/// runs. Read plays it. Touch writes a parameter while it is held and reads
/// again when it is let go. Latch writes from the first touch until the
/// One recording on an audio track: a file laid from `start_tick`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Take {
    pub name: String,
    pub path: String,
    pub start_tick: u64,
    pub length_tick: u64,
}

/// A span of the comp playing one take: `[from_tick, to_tick)`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompSegment {
    pub from_tick: u64,
    pub to_tick: u64,
    pub take: usize,
}

/// Where a track's notes go over MIDI: a port by name, and a channel
/// from 1 to 16.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MidiOutRoute {
    pub port: String,
    pub channel: u8,
}

/// Which clocks the transport sends and follows.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncSettings {
    /// Send MIDI clock, start, stop, continue and song position.
    #[serde(default)]
    pub clock_out: bool,
    /// Send MIDI time code quarter frames.
    #[serde(default)]
    pub mtc_out: bool,
    /// MTC frame rate: 24, 25 or 30.
    #[serde(default = "SyncSettings::default_fps")]
    pub mtc_fps: u8,
    /// Follow an incoming MIDI clock: its tempo, start, stop and position.
    #[serde(default)]
    pub clock_in: bool,
    /// Share tempo, phase and start/stop with Ableton Link peers.
    #[serde(default)]
    pub link: bool,
}

impl SyncSettings {
    fn default_fps() -> u8 { 25 }
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self { clock_out: false, mtc_out: false, mtc_fps: 25, clock_in: false, link: false }
    }
}

/// transport stops. Write does what Latch does and overwrites every lane the
/// track already has from where the pass starts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AutomationMode {
    Off,
    #[default]
    Read,
    Write,
    Touch,
    Latch,
}

impl AutomationMode {
    pub fn letter(self) -> &'static str {
        match self {
            AutomationMode::Off => "-",
            AutomationMode::Read => "R",
            AutomationMode::Write => "W",
            AutomationMode::Touch => "T",
            AutomationMode::Latch => "L",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AutomationMode::Off => "Off",
            AutomationMode::Read => "Read",
            AutomationMode::Write => "Write",
            AutomationMode::Touch => "Touch",
            AutomationMode::Latch => "Latch",
        }
    }

    /// The next mode round, for a button that cycles.
    pub fn next(self) -> Self {
        match self {
            AutomationMode::Off => AutomationMode::Read,
            AutomationMode::Read => AutomationMode::Touch,
            AutomationMode::Touch => AutomationMode::Latch,
            AutomationMode::Latch => AutomationMode::Write,
            AutomationMode::Write => AutomationMode::Off,
        }
    }

    pub fn writes(self) -> bool {
        matches!(self, AutomationMode::Write | AutomationMode::Touch | AutomationMode::Latch)
    }

    pub fn reads(self) -> bool {
        !matches!(self, AutomationMode::Off)
    }
}

impl Default for TrackConfig {
    fn default() -> Self {
        Self {
            name:          "Track".to_string(),
            kind:          TrackKind::Melodic,
            engine:        TrackSource::Empty,
            timeline:      Vec::new(),
            session_clips: vec![None; 8],
            volume:        0.8,
            pan:           0.0,
            mute:          false,
            solo:          false,
            sends:         default_sends(),
            color:         [80, 160, 200],
            automations:   Vec::new(),
            fx:            ChainSpec::default(),
            fx_slots:      Vec::new(),
            midi_fx_slots: Vec::new(),
            vst3_state:    Vec::new(),
            vst3_inserts:  Vec::new(),
            engine_state:  Vec::new(),
            internal_seq:  true,
            output_group:  None,
            audio_input:   None,
            scale_lock:    false,
            frozen_original: None,
            track_automation: Vec::new(),
            automation_mode: AutomationMode::default(),
            midi_out: None,
            vca: None,
            xfade: 0,
            folder: None,
            surround: crate::surround::SurroundPos::default(),
            takes: Vec::new(),
            comp: Vec::new(),
        }
    }
}

// ── Per-track VST3 insert effect state (serializable) ────────────────────────

/// Serializable snapshot of one VST3 insert EFFECT on a sequencer track.
/// `position` is the index in the track's insert segment list, where the
/// built-in EffectsChain is one segment — so a VST3 saved at position 0
/// runs BEFORE the built-in FX and 1+ runs after; round-trips exactly.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Vst3InsertState {
    /// `.vst3` bundle or DLL path the plugin was loaded from.
    pub path:     String,
    /// Opaque component state blob (IComponent::getState).
    #[serde(default)]
    pub state:    Vec<u8>,
    /// Index in the track's insert segment vec (see above).
    #[serde(default)]
    pub position: u8,
}

// ── Per-track insert FX slot state (serializable) ────────────────────────────

/// Number of insert-FX slots a sequencer track has. Named here so the rack
/// backends, the meter-mirror arrays in `SeqState`, and every loop that walks a
/// track's slots stay in lockstep; a chain description truncates to it.
pub const TRACK_FX_SLOTS: usize = 8;

/// The track a slot is keyed from, as its `sidechain` names it.
pub fn sidechain_track(slot: &SlotSpec) -> Option<u16> {
    slot.sidechain.as_deref()?.strip_prefix("track:")?.parse().ok()
}

pub fn set_sidechain_track(slot: &mut SlotSpec, track: Option<u16>) {
    slot.sidechain = track.map(phonix_legacy::sidechain_name);
}

// ── Per-track MIDI FX slot state (serializable) ───────────────────────────────

/// Serializable snapshot of one MIDI FX slot on a sequencer track.
/// `fx_type` is the `MidiFxType` discriminant (0 = None/empty).
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct MidiFxSlotState {
    pub fx_type: u8,
    pub enabled: bool,
    #[serde(default)]
    pub params:  Vec<(u8, f32)>,   // sparse (pid, value) pairs
}

// ── Launch mode ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq, Default)]
pub enum LaunchMode {
    #[default]
    Loop,
    OneShot,
}

/// Per-pattern launch-quantize. Controls how QueueClip waits for a
/// grid boundary before swapping the active pattern. Default `Bar`
/// matches the pre-feature behaviour (flush on every bar boundary).
#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq, Default)]
pub enum LaunchQuantize {
    /// Fire on the next block — no quantize wait.
    Immediate,
    /// Fire on the next beat boundary.
    Beat,
    #[default]
    Bar,
    TwoBars,
    FourBars,
}

impl LaunchQuantize {
    /// Compact label used by the pattern browser dropdown.
    pub fn label(self) -> &'static str {
        match self {
            LaunchQuantize::Immediate => "Now",
            LaunchQuantize::Beat      => "Beat",
            LaunchQuantize::Bar       => "1 Bar",
            LaunchQuantize::TwoBars   => "2 Bars",
            LaunchQuantize::FourBars  => "4 Bars",
        }
    }
    pub fn all() -> &'static [LaunchQuantize] {
        &[Self::Immediate, Self::Beat, Self::Bar, Self::TwoBars, Self::FourBars]
    }
}

// ── Clip placement ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Copy, Default)]
pub struct ClipPlacement {
    pub pattern_id:  usize,
    pub start_tick:  u64,
    /// 0 = use pattern's natural length
    pub length_tick: u64,
    // ── Audio-clip edit window (ignored by MIDI clips) ──────────────────────
    // All `#[serde(default)]` so existing .phx sessions round-trip unchanged
    // and the struct stays `Copy` (no String here). Units: `in_point_samples`
    // is a CLIP-NATIVE sample offset (trim in-point / split); fades in ms;
    // gain in dB on top of the note velocity (0 dB default = current behaviour).
    #[serde(default)] pub in_point_samples: u64,
    #[serde(default)] pub clip_gain_db:     f32,
    #[serde(default)] pub fade_in_ms:       f32,
    #[serde(default)] pub fade_out_ms:      f32,
    /// Time-stretch ratio (1.0 = native speed; 2.0 = twice as long, same
    /// pitch — offline WSOLA). 0.0 (old sessions / Default) reads as 1.0
    /// via `stretch_ratio()`.
    #[serde(default)] pub stretch:          f32,
    /// Tempo-sync: the material's native BPM. When set (> 0), the clip
    /// stretches by source_bpm / session_bpm and FOLLOWS tempo changes;
    /// overrides the manual `stretch` ratio. 0 = off.
    #[serde(default)] pub source_bpm:       f32,
    /// How the clip is stretched: by grains that keep the pitch, by its
    /// attacks kept whole, or by resampling that moves the pitch.
    #[serde(default)] pub warp:             WarpMode,
}

/// How a clip is stretched.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WarpMode {
    /// Grains overlapped by similarity: pitch kept, for most material.
    #[default]
    Complex,
    /// Attacks kept whole and moved to their new places: drums.
    Beats,
    /// Resampled: the pitch moves with the length, as tape does.
    RePitch,
}

impl WarpMode {
    pub const ALL: [WarpMode; 3] = [WarpMode::Complex, WarpMode::Beats, WarpMode::RePitch];
    pub fn name(self) -> &'static str {
        match self { WarpMode::Complex => "Complex", WarpMode::Beats => "Beats", WarpMode::RePitch => "Re-Pitch" }
    }
    pub fn code(self) -> u8 { self as u8 }
}

impl ClipPlacement {
    /// Effective stretch ratio: unset/zero means native speed.
    pub fn stretch_ratio(&self) -> f32 {
        if self.stretch <= 0.01 { 1.0 } else { self.stretch.clamp(0.25, 4.0) }
    }

    /// Ratio including tempo-sync: source_bpm (when set) wins over the
    /// manual ratio and tracks the session tempo.
    pub fn effective_stretch(&self, session_bpm: f32) -> f32 {
        if self.source_bpm > 1.0 && session_bpm > 1.0 {
            (self.source_bpm / session_bpm).clamp(0.25, 4.0)
        } else {
            self.stretch_ratio()
        }
    }
}

// ── Pattern ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Pattern {
    pub id:          usize,
    pub name:        String,
    pub length_bars: u32,
    pub data:        PatternData,
    #[serde(default)]
    pub launch_mode: LaunchMode,
    /// Per-clip color [R,G,B]. [0,0,0] means inherit track color.
    #[serde(default)]
    pub color:       [u8; 3],
    /// Per-pattern launch quantize. Default `Bar` keeps the historical
    /// behaviour of waiting for the next bar boundary.
    #[serde(default)]
    pub launch_quantize: LaunchQuantize,
    /// Per-note articulation / control events fired to the track engine at their
    /// tick during playback (guitar slides + power-chord toggles, slap pop/fret
    /// buzz, pitch bend, dist drive...). Empty for ordinary patterns; lets an
    /// `.phx` replay the algorithmic trance layers' articulation exactly.
    #[serde(default)]
    pub artic: Vec<ArticPoint>,
    /// Per-note expression curves, sent to a hosted plugin that takes
    /// them while the note sounds.
    #[serde(default)]
    pub note_expr: Vec<NoteExpr>,
    /// What the clip does once it has played its loops in the session.
    #[serde(default)]
    pub follow: FollowAction,
    /// The marks of interpretation written on the score.
    #[serde(default)]
    pub marks: Vec<ScoreMark>,
}

/// A mark of interpretation on a pattern: at `tick`, and up to `end` for
/// the kinds that span.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ScoreMark {
    pub tick: u64,
    #[serde(default)]
    pub end: u64,
    pub kind: MarkKind,
}

/// The kinds of mark: a dynamic level from ppp to fff, hairpins, a phrase
/// slur, an accent, a fermata, a ritenuto and the return to tempo.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Dynamic(u8),
    Crescendo,
    Diminuendo,
    Phrase,
    Accent,
    Fermata,
    Ritenuto,
    ATempo,
}

/// The dynamic levels, softest first.
pub const DYNAMIC_NAMES: [&str; 8] = ["ppp", "pp", "p", "mp", "mf", "f", "ff", "fff"];

/// The velocity a dynamic level plays at.
pub fn dynamic_velocity(level: u8) -> u8 {
    const V: [u8; 8] = [20, 33, 46, 60, 74, 88, 102, 116];
    V[(level as usize).min(7)]
}

impl MarkKind {
    /// The marks a tool can place, dynamics from ppp to fff first.
    pub fn all() -> Vec<MarkKind> {
        let mut v: Vec<MarkKind> = (0..8).map(MarkKind::Dynamic).collect();
        v.extend([MarkKind::Crescendo, MarkKind::Diminuendo, MarkKind::Phrase, MarkKind::Accent, MarkKind::Fermata, MarkKind::Ritenuto, MarkKind::ATempo]);
        v
    }

    pub fn label(self) -> &'static str {
        match self {
            MarkKind::Dynamic(l) => DYNAMIC_NAMES[(l as usize).min(7)],
            MarkKind::Crescendo => "cresc.",
            MarkKind::Diminuendo => "dim.",
            MarkKind::Phrase => "phrase",
            MarkKind::Accent => ">",
            MarkKind::Fermata => "fermata",
            MarkKind::Ritenuto => "rit.",
            MarkKind::ATempo => "a tempo",
        }
    }

    /// Whether the mark runs from its tick to its end.
    pub fn spans(self) -> bool {
        matches!(self, MarkKind::Crescendo | MarkKind::Diminuendo | MarkKind::Phrase | MarkKind::Fermata | MarkKind::Ritenuto)
    }
}

/// What a session clip does when its loops are done.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FollowKind {
    #[default]
    None,
    Stop,
    Next,
    Previous,
    First,
    Last,
    Any,
    Other,
    Again,
}

impl FollowKind {
    pub const ALL: [FollowKind; 9] = [FollowKind::None, FollowKind::Stop, FollowKind::Next, FollowKind::Previous, FollowKind::First, FollowKind::Last, FollowKind::Any, FollowKind::Other, FollowKind::Again];

    pub fn name(self) -> &'static str {
        match self {
            FollowKind::None => "none", FollowKind::Stop => "stop", FollowKind::Next => "next", FollowKind::Previous => "previous",
            FollowKind::First => "first", FollowKind::Last => "last", FollowKind::Any => "any", FollowKind::Other => "other", FollowKind::Again => "again",
        }
    }
}

/// After `after_loops` loops, action `a` with `chance_a` percent, `b`
/// otherwise.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FollowAction {
    pub after_loops: u32,
    pub a: FollowKind,
    pub b: FollowKind,
    pub chance_a: u8,
}

impl Default for FollowAction {
    fn default() -> Self { Self { after_loops: 1, a: FollowKind::None, b: FollowKind::None, chance_a: 100 } }
}

impl FollowAction {
    pub fn is_set(&self) -> bool { self.a != FollowKind::None || self.b != FollowKind::None }

    /// The slot to play after slot `current`, among the slots that hold
    /// a clip, for a draw from 0 to 99; None to stop, `Some(current)` to
    /// go on.
    pub fn resolve(&self, current: usize, filled: &[usize], draw: u8) -> Option<usize> {
        let kind = if draw < self.chance_a { self.a } else { self.b };
        let pos = filled.iter().position(|&s| s == current);
        let n = filled.len();
        match kind {
            FollowKind::None | FollowKind::Again => Some(current),
            FollowKind::Stop => None,
            FollowKind::Next => pos.map(|p| filled[(p + 1) % n]).or(filled.first().copied()),
            FollowKind::Previous => pos.map(|p| filled[(p + n - 1) % n]).or(filled.first().copied()),
            FollowKind::First => filled.first().copied(),
            FollowKind::Last => filled.last().copied(),
            FollowKind::Any => if n == 0 { None } else { Some(filled[draw as usize % n]) },
            FollowKind::Other => {
                let others: Vec<usize> = filled.iter().copied().filter(|&s| s != current).collect();
                if others.is_empty() { Some(current) } else { Some(others[draw as usize % others.len()]) }
            }
        }
    }
}

/// A scene of the session view: a name, and a tempo and meter it sets
/// when launched.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Scene {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bpm: Option<f32>,
    #[serde(default)]
    pub time_sig: Option<(u8, u8)>,
}

/// What a note's expression curve drives, as CLAP names them.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExprKind { Volume, Pan, Tuning, Vibrato, Expression, Brightness, Pressure }

impl ExprKind {
    pub const ALL: [ExprKind; 7] = [ExprKind::Volume, ExprKind::Pan, ExprKind::Tuning, ExprKind::Vibrato, ExprKind::Expression, ExprKind::Brightness, ExprKind::Pressure];

    pub fn name(self) -> &'static str {
        match self {
            ExprKind::Volume => "Volume", ExprKind::Pan => "Pan", ExprKind::Tuning => "Tuning", ExprKind::Vibrato => "Vibrato",
            ExprKind::Expression => "Expression", ExprKind::Brightness => "Brightness", ExprKind::Pressure => "Pressure",
        }
    }

    /// The CLAP expression id.
    pub fn clap_id(self) -> i32 {
        match self {
            ExprKind::Volume => 0, ExprKind::Pan => 1, ExprKind::Tuning => 2, ExprKind::Vibrato => 3,
            ExprKind::Expression => 4, ExprKind::Brightness => 5, ExprKind::Pressure => 6,
        }
    }

    /// A lane value from 0 to 1 in the expression's own units: tuning
    /// spans an octave either way, the rest stay from 0 to 1.
    pub fn units(self, v: f32) -> f64 {
        match self {
            ExprKind::Tuning => (v as f64 * 24.0) - 12.0,
            _ => v.clamp(0.0, 1.0) as f64,
        }
    }

    /// The lane value the expression rests at.
    pub fn rest(self) -> f32 {
        match self { ExprKind::Volume | ExprKind::Expression => 1.0, ExprKind::Pan | ExprKind::Tuning => 0.5, _ => 0.0 }
    }
}

/// A note's expression curve: the note by its start tick and pitch, the
/// kind, and points in ticks from the note's start with values 0 to 1.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NoteExpr {
    pub tick: u64,
    pub pitch: u8,
    pub kind: ExprKind,
    pub points: Vec<AutoPoint>,
}

impl NoteExpr {
    /// The value `rel` ticks into the note: held before the first point and
    /// after the last, linear between.
    pub fn value_at(&self, rel: u64) -> Option<f32> {
        let lane = AutomationLane { relative: false, param_id: String::new(), points: self.points.clone() };
        lane.value_at(rel)
    }
}

/// A timed articulation/control event in a pattern (pattern-relative tick).
#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq)]
pub struct ArticPoint {
    pub tick:  u64,
    pub event: ArticEvent,
}

/// Engine-agnostic per-note articulation. At playback each is dispatched to the
/// track engine's command channel (mapped per engine; ignored if unsupported).
#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq)]
pub enum ArticEvent {
    PitchBend(f32),   // semitones
    SlideTime(f32),   // s — guitar native portamento window
    PowerChord(bool), // guitar
    PalmMute(f32),    // guitar
    SlapPop(f32),     // bass thumb<->pop
    FretBuzz(f32),    // bass
    DistDrive(f32),   // bass/guitar drive
    DistType(u8),     // bass dist type
    /// Sustain pedal, for engines that have dampers. A piano's pedalling
    /// belongs in the pattern rather than in a live CC lane: it is part of the
    /// piece, and a session has to replay it exactly.
    SustainPedal(bool),
}

/// Ticks in one bar of `time_sig`.
///
impl Pattern {
    /// Pattern length in ticks. `bar_ticks` comes from the session's time
    /// signature ([`Session::bar_ticks`]); a pattern does not know its own
    /// metre, since the session owns it.
    pub fn length_ticks(&self, bar_ticks: u64) -> u64 {
        self.length_bars as u64 * bar_ticks
    }

    pub fn new_piano_roll(id: usize, name: &str, bars: u32) -> Self {
        Self { id, name: name.to_string(), length_bars: bars,
               data: PatternData::PianoRoll(vec![]),
               launch_mode: LaunchMode::Loop, color: [0; 3],
               launch_quantize: LaunchQuantize::default(), artic: Vec::new(), note_expr: Vec::new(), follow: Default::default(), marks: Vec::new() }
    }

    pub fn new_steps(id: usize, name: &str, bars: u32, step_count: usize, rows: Vec<StepRow>) -> Self {
        let steps = vec![vec![StepCell::default(); step_count]; rows.len()];
        Self {
            id, name: name.to_string(), length_bars: bars,
            data: PatternData::Steps(StepPattern { steps, step_count, rows, groove_idx: 0 }),
            launch_mode: LaunchMode::Loop, color: [0; 3],
            launch_quantize: LaunchQuantize::default(), artic: Vec::new(), note_expr: Vec::new(), follow: Default::default(), marks: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum PatternData {
    PianoRoll(Vec<MidiNote>),
    Steps(StepPattern),
}

/// Step condition: controls when a step fires beyond simple probability.
#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq, Default)]
pub enum StepCondition {
    #[default]
    Always,
    /// Fire every Nth cycle (2, 3, 4, 8)
    EveryN(u8),
    /// Skip the first occurrence, play all subsequent
    SkipFirst,
    /// Play only the first time through
    FirstOnly,
    /// Only fire when fill mode is active
    Fill,
}

/// Dependency mode for visual cabling between steps.
#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq, Default)]
pub enum StepDepMode {
    /// Fire this step only if the source step fired
    #[default]
    PlayIfFired,
    /// Fire this step only if the source step did NOT fire
    PlayIfNotFired,
}

/// A dependency link from one step to another within the same pattern.
#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq, Default)]
pub struct StepDep {
    /// Source row index
    pub src_row:  usize,
    /// Source step index
    pub src_step: usize,
    /// Dependency mode
    pub mode:     StepDepMode,
}

// ── Step pattern ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StepPattern {
    /// steps[row][step] — row = drum sound, step = step index
    pub steps:      Vec<Vec<StepCell>>,
    pub step_count: usize,
    pub rows:       Vec<StepRow>,
    /// Index into generative::GROOVE_TEMPLATES (0 = Straight / no groove)
    #[serde(default)]
    pub groove_idx: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug, Copy)]
pub struct StepCell {
    pub on:       bool,
    pub velocity: u8,   // 0 = use row default (100)
    #[serde(default = "StepCell::default_probability")]
    pub probability: u8,  // 1–100, default 100 = always fire
    /// Step condition: when this step fires beyond probability
    #[serde(default)]
    pub condition: StepCondition,
    /// Random velocity scatter ±amount (0 = off)
    #[serde(default)]
    pub vel_random: u8,
    /// Alternate pitches (MIDI note, 0 = unused). Randomly selected per trigger.
    #[serde(default)]
    pub alt_pitches: [u8; 3],
    /// Weights for alt pitch selection (0 = unused, higher = more likely)
    #[serde(default)]
    pub alt_weights: [u8; 3],
    /// Optional dependency on another step (visual cable source)
    #[serde(default)]
    pub depends_on: Option<StepDep>,
}

impl StepCell {
    fn default_probability() -> u8 { 100 }
}

impl Default for StepCell {
    fn default() -> Self {
        Self {
            on: false, velocity: 0, probability: 100,
            condition: StepCondition::Always, vel_random: 0,
            alt_pitches: [0; 3], alt_weights: [0; 3],
            depends_on: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StepRow {
    pub pitch: u8,
    pub name:  String,
}

impl StepRow {
    pub fn new(pitch: u8, name: &str) -> Self { Self { pitch, name: name.to_string() } }
}

/// Default drum kit rows (GM drum map subset)
pub fn default_drum_rows() -> Vec<StepRow> {
    vec![
        StepRow::new(36, "Kick"),
        StepRow::new(38, "Snare"),
        StepRow::new(42, "HH Closed"),
        StepRow::new(46, "HH Open"),
        StepRow::new(49, "Crash"),
        StepRow::new(51, "Ride"),
        StepRow::new(39, "Clap"),
        StepRow::new(37, "Rim"),
    ]
}

// ── Automation ────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AutomationLane {
    pub param_id: String,
    pub points:   Vec<AutoPoint>,
    /// Scales the parameter's own value instead of setting it: 1 leaves it,
    /// 0 takes it away; pan and the surround place shift by the lane.
    #[serde(default)]
    pub relative: bool,
}

impl AutomationLane {
    /// The lane's value at `tick`: held before the first point and after the
    /// last, linear between two.
    pub fn value_at(&self, tick: u64) -> Option<f32> {
        let pts = &self.points;
        if pts.is_empty() {
            return None;
        }
        Some(if tick <= pts[0].tick {
            pts[0].value
        } else if tick >= pts[pts.len() - 1].tick {
            pts[pts.len() - 1].value
        } else {
            let idx = pts.partition_point(|p| p.tick <= tick).saturating_sub(1);
            let p0 = &pts[idx];
            let p1 = &pts[(idx + 1).min(pts.len() - 1)];
            if p1.tick == p0.tick {
                p0.value
            } else {
                let t = (tick - p0.tick) as f32 / (p1.tick - p0.tick) as f32;
                p0.value + (p1.value - p0.value) * t
            }
        })
    }

    /// Write `value` at `tick` over whatever the lane held in `(from, tick]`:
    /// how a pass overwrites the span it moves across.
    pub fn write(&mut self, from: u64, tick: u64, value: f32) {
        self.points.retain(|p| p.tick <= from.min(tick) || p.tick > tick);
        self.points.push(AutoPoint { tick, value });
        self.points.sort_by_key(|p| p.tick);
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq)]
pub struct AutoPoint {
    pub tick:  u64,
    pub value: f32,
}

// ── Mixer state ───────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MixerState {
    pub master_volume: f32,
    #[serde(default = "default_returns")]
    pub send_returns:  Vec<SendReturn>,
    /// The master bus chain.
    #[serde(default)]
    pub master_fx: ChainSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub master_fx_slots: Vec<phonix_legacy::FxSlotState>,
    /// Pre-master "mix bus" volume — output trim before the master
    /// stage. Defaults to unity. Saved patches without this field
    /// fall back to 1.0 so they sound identical to before.
    #[serde(default = "MixerState::default_mix_volume")]
    pub mix_volume: f32,
    /// Pre-master mix-bus insert FX chain. Up to 8 slots, identical
    /// catalogue to the master chain. Glue compression / tonal
    /// balance lives here; the master chain stays for final
    /// ceiling / limiting / dithering.
    #[serde(default)]
    pub mix_fx: ChainSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mix_fx_slots: Vec<phonix_legacy::FxSlotState>,
    /// Group (submix) buses. A track with `output_group: Some(g)` routes into
    /// `group_buses[g]` instead of the master; the group applies its FX chain to the
    /// SUMMED stereo of its members, then sums to the master. Enables a multi-FX submix
    /// (e.g. a hard-panned guitar double-track + chorus/delay/hall on the sum = exact width).
    #[serde(default)]
    pub group_buses: Vec<GroupBusConfig>,
    /// VCA faders: each scales the volume of the tracks assigned to it,
    /// without carrying audio.
    #[serde(default)]
    pub vcas: Vec<VcaConfig>,
    /// The speakers the mix is laid on.
    #[serde(default)]
    pub layout: crate::surround::SpeakerLayout,
    /// Crossfader position: 0 all A, 1 all B.
    #[serde(default = "default_crossfader")]
    pub crossfader: f32,
}

fn default_crossfader() -> f32 { 0.5 }

/// A VCA fader: a name and a gain from 0 to 1 applied to its tracks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VcaConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default = "VcaConfig::default_level")]
    pub level: f32,
}

impl VcaConfig {
    fn default_level() -> f32 { 1.0 }
}

/// The gain the VCAs apply to a track: its VCA's level, or unity.
pub fn vca_gain_of(vcas: &[VcaConfig], track: &TrackConfig) -> f32 {
    track.vca.and_then(|v| vcas.get(v as usize)).map(|v| v.level.clamp(0.0, 1.0)).unwrap_or(1.0)
}

/// The gain the crossfader at `pos` applies to a track on `side`: equal
/// power between the sides, unity for a track on neither.
pub fn xfade_gain(side: u8, pos: f32) -> f32 {
    let a = pos.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    match side {
        1 => a.cos(),
        2 => a.sin(),
        _ => 1.0,
    }
}

/// Config for one group (submix) bus: an FX chain + output level applied to the sum
/// of its member tracks before it folds into the master.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct GroupBusConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default = "GroupBusConfig::default_volume")]
    pub volume: f32,
    #[serde(default)]
    pub fx: ChainSpec,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fx_slots: Vec<phonix_legacy::FxSlotState>,
}

impl GroupBusConfig {
    fn default_volume() -> f32 { 1.0 }
}

impl MixerState {
    fn default_mix_volume() -> f32 { 1.0 }
}

/// Most send returns a session can hold.
pub const MAX_SENDS: usize = 12;
/// Send returns of a new session.
pub const DEFAULT_SENDS: usize = 4;

fn default_sends() -> Vec<f32> {
    vec![0.0; DEFAULT_SENDS]
}

fn default_returns() -> Vec<SendReturn> {
    MixerState::default().send_returns
}

#[derive(Serialize, Deserialize, Clone, Debug, Copy)]
pub struct SendReturn {
    pub kind:   SendKind,
    pub mix:    f32,
    pub size:   f32,
    #[serde(default = "SendReturn::default_param2")]
    pub param2: f32,
    /// Only meaningful when `kind == SendKind::Reverb`. Picks between
    /// the algorithmic Schroeder bus and the convolution bus. Old
    /// sessions without the field default to `Algorithmic` so existing
    /// .phx files load with their original sound.
    #[serde(default)]
    pub reverb_kind: crate::ReverbKind,
    /// Only meaningful when `kind == SendKind::Reverb && reverb_kind == Convolution`.
    /// Index into `IrKind::ALL` (0..5): 0 Cathedral / 1 Concrete /
    /// 2 Plate / 3 Wooden Hall / 4 Spring / 5 Ambient Room.
    #[serde(default)]
    pub ir_kind: u8,
    /// Reverb decay (0..1). Only for `kind == Reverb`. Defaults to the algorithmic reverb's
    /// built-in default (0.5) so old .aeth files load with their original sound.
    #[serde(default = "SendReturn::default_decay")]
    pub decay: f32,
    /// Reverb pre-delay seconds (0..0.5). Only for `kind == Reverb`. Default 0.0 = prior behaviour.
    #[serde(default)]
    pub predelay: f32,
    /// Reverb algorithm type index (ReverbType::from_index): 0 Hall .. . Only for `kind == Reverb`.
    /// Default 0 (Hall) = the reverb's built-in default, so old .aeth files are unchanged.
    #[serde(default)]
    pub rtype: u8,
}

impl SendReturn {
    /// A fresh algorithmic reverb return.
    pub fn reverb() -> Self {
        SendReturn { kind: SendKind::Reverb, mix: 1.0, size: 0.7, param2: 0.5, reverb_kind: crate::ReverbKind::Algorithmic, ir_kind: 0, decay: 0.5, predelay: 0.0, rtype: 0 }
    }
}

impl SendReturn {
    fn default_param2() -> f32 { 0.5 }
    fn default_decay() -> f32 { 0.5 }
}

fn default_internal_seq() -> bool { true }

#[derive(Serialize, Deserialize, Clone, Debug, Copy, PartialEq)]
pub enum SendKind { Reverb, Delay, Chorus, Phaser, Flanger, Filter, LoFi }

impl SendKind {
    pub fn all() -> &'static [SendKind] {
        &[Self::Reverb, Self::Delay, Self::Chorus, Self::Phaser, Self::Flanger, Self::Filter, Self::LoFi]
    }
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Reverb  => "Reverb",
            Self::Delay   => "Delay",
            Self::Chorus  => "Chorus",
            Self::Phaser  => "Phaser",
            Self::Flanger => "Flanger",
            Self::Filter  => "Filter",
            Self::LoFi    => "Lo-Fi",
        }
    }
    /// Label for the primary knob (size).
    pub fn size_label(self) -> &'static str {
        match self {
            Self::Reverb  => "Size",
            Self::Delay   => "Feedback",
            Self::Chorus  => "Depth",
            Self::Phaser  => "Rate",
            Self::Flanger => "Rate",
            Self::Filter  => "Cutoff",
            Self::LoFi    => "Crush",
        }
    }
    /// Label for the secondary knob (param2).
    pub fn param2_label(self) -> &'static str {
        match self {
            Self::Reverb  => "Damping",
            Self::Delay   => "Time",
            Self::Chorus  => "Rate",
            Self::Phaser  => "Feedback",
            Self::Flanger => "Feedback",
            Self::Filter  => "Resonance",
            Self::LoFi    => "Noise",
        }
    }
}

impl Default for MixerState {
    fn default() -> Self {
        Self {
            master_volume: 0.85,
            send_returns: vec![
                SendReturn { kind: SendKind::Reverb, mix: 1.0, size: 0.7, param2: 0.5, reverb_kind: crate::ReverbKind::Algorithmic, ir_kind: 0, decay: 0.5, predelay: 0.0, rtype: 0 },
                SendReturn { kind: SendKind::Delay,  mix: 1.0, size: 0.5, param2: 0.5, reverb_kind: crate::ReverbKind::Algorithmic, ir_kind: 0, decay: 0.5, predelay: 0.0, rtype: 0 },
                SendReturn { kind: SendKind::Chorus, mix: 1.0, size: 0.5, param2: 0.5, reverb_kind: crate::ReverbKind::Algorithmic, ir_kind: 0, decay: 0.5, predelay: 0.0, rtype: 0 },
                SendReturn { kind: SendKind::Reverb, mix: 1.0, size: 0.9, param2: 0.5, reverb_kind: crate::ReverbKind::Algorithmic, ir_kind: 0, decay: 0.5, predelay: 0.0, rtype: 0 },
            ],
            master_fx: ChainSpec::default(),
            master_fx_slots: Vec::new(),
            mix_volume: 1.0,
            mix_fx: ChainSpec::default(),
            mix_fx_slots: Vec::new(),
            group_buses: Vec::new(),
            vcas: Vec::new(),
            layout: crate::surround::SpeakerLayout::Stereo,
            crossfader: 0.5,
        }
    }
}

// ── Harmony Center ────────────────────────────────────────────────────────────

/// Session-level harmonic center: a chord progression that advances with the transport.
/// Any track with a ChordToneSnap MIDI FX will snap its notes to the current chord tones.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HarmonyCenter {
    pub root:           u8,        // 0=C .. 11=B
    pub scale_idx:      usize,     // index into generative::SCALE_NAMES
    pub progression:    Vec<u8>,   // 1-based scale degrees, e.g. [1,4,5,1]
    pub bars_per_chord: u32,
    pub add_seventh:    bool,
}

impl Default for HarmonyCenter {
    fn default() -> Self {
        Self { root: 0, scale_idx: 1, progression: vec![1, 4, 5, 1], bars_per_chord: 1, add_seventh: false }
    }
}

// ── Arranger Sections ─────────────────────────────────────────────────────────

/// One named arrangement section (Intro / Verse / Chorus …) spanning a tick
/// range. Moving, duplicating or deleting a section carries everything inside
/// its span — clips, chord events and conductor marks — as one atomic edit
/// (see `Session::shift_range` / `remove_range`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Section {
    pub start_tick: u64,
    pub len_tick:   u64,
    pub name:       String,
    pub color:      [u8; 3],
}

impl Section {
    pub fn end_tick(&self) -> u64 {
        self.start_tick + self.len_tick
    }
    pub fn covers(&self, tick: u64) -> bool {
        tick >= self.start_tick && tick < self.end_tick()
    }
}

impl Session {
    /// Shift every timeline clip, chord event and conductor mark whose start
    /// lies in `[from, from + span)` by `delta` ticks (saturating at 0).
    /// The core move used by section drag/duplicate. Callers snapshot the
    /// session for undo around this.
    fn default_punch_in() -> u32 { 1 }
    fn default_punch_out() -> u32 { 2 }

    /// The punch range in ticks, from the bar numbers.
    pub fn punch_ticks(&self) -> (u64, u64) {
        (self.tick_of_bar(self.punch_in_bar.saturating_sub(1)),
         self.tick_of_bar(self.punch_out_bar.saturating_sub(1)))
    }

    /// Replace the markers, kept sorted by tick.
    pub fn set_markers(&mut self, mut markers: Vec<Marker>) {
        markers.sort_by_key(|m| m.tick);
        self.markers = markers;
    }

    /// The marker before `tick`, or the one at it when `before` is false
    /// and it sits exactly there.
    pub fn marker_before(&self, tick: u64) -> Option<&Marker> {
        self.markers.iter().rev().find(|m| m.tick < tick)
    }

    pub fn marker_after(&self, tick: u64) -> Option<&Marker> {
        self.markers.iter().find(|m| m.tick > tick)
    }

    pub fn shift_range(&mut self, from: u64, span: u64, delta: i64) {
        let end = from + span;
        let mv = |t: u64| -> u64 {
            if t >= from && t < end {
                (t as i64 + delta).max(0) as u64
            } else {
                t
            }
        };
        for track in &mut self.tracks {
            for pl in &mut track.timeline {
                pl.start_tick = mv(pl.start_tick);
            }
            for lane in &mut track.track_automation {
                for p in &mut lane.points {
                    p.tick = mv(p.tick);
                }
                lane.points.sort_by_key(|p| p.tick);
            }
        }
        for m in &mut self.markers { m.tick = mv(m.tick); }
        self.markers.sort_by_key(|m| m.tick);
        for ev in &mut self.chord_track.events {
            ev.start_tick = mv(ev.start_tick);
        }
        self.chord_track.sort();
        for m in &mut self.conductor.events {
            m.start_tick = mv(m.start_tick);
        }
        self.conductor.sort();
        self.shift_time_maps(from, span, delta);
    }

    /// Delete everything (clips/chords/marks) starting in `[from, from+span)`
    /// and pull later content back by `span` (a "cut time" ripple delete).
    pub fn remove_range(&mut self, from: u64, span: u64) {
        let end = from + span;
        for track in &mut self.tracks {
            track.timeline.retain(|pl| !(pl.start_tick >= from && pl.start_tick < end));
            for pl in &mut track.timeline {
                if pl.start_tick >= end {
                    pl.start_tick -= span;
                }
            }
            for lane in &mut track.track_automation {
                lane.points.retain(|p| !(p.tick >= from && p.tick < end));
                for p in &mut lane.points {
                    if p.tick >= end { p.tick -= span; }
                }
            }
        }
        self.markers.retain(|m| !(m.tick >= from && m.tick < end));
        for m in &mut self.markers {
            if m.tick >= end { m.tick -= span; }
        }
        self.chord_track.events.retain(|e| !(e.start_tick >= from && e.start_tick < end));
        for e in &mut self.chord_track.events {
            if e.start_tick >= end { e.start_tick -= span; }
        }
        self.chord_track.sort();
        self.conductor.events.retain(|e| !(e.start_tick >= from && e.start_tick < end));
        for e in &mut self.conductor.events {
            if e.start_tick >= end { e.start_tick -= span; }
        }
        self.conductor.sort();
        self.remove_time_maps(from, span);
    }
}

// ── Chord Track ───────────────────────────────────────────────────────────────

/// Chord quality for a `ChordEvent`. `intervals()` are semitones from the root.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChordQuality {
    Maj, Min, Dim, Aug, Sus2, Sus4,
    Maj7, Min7, Dom7, Min7b5, Dim7, MinMaj7,
    Maj6, Min6, Add9, Power,
}

impl ChordQuality {
    pub const ALL: [Self; 16] = [
        Self::Maj, Self::Min, Self::Dim, Self::Aug, Self::Sus2, Self::Sus4,
        Self::Maj7, Self::Min7, Self::Dom7, Self::Min7b5, Self::Dim7, Self::MinMaj7,
        Self::Maj6, Self::Min6, Self::Add9, Self::Power,
    ];

    pub fn intervals(self) -> &'static [u8] {
        match self {
            Self::Maj => &[0, 4, 7],
            Self::Min => &[0, 3, 7],
            Self::Dim => &[0, 3, 6],
            Self::Aug => &[0, 4, 8],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::Maj7 => &[0, 4, 7, 11],
            Self::Min7 => &[0, 3, 7, 10],
            Self::Dom7 => &[0, 4, 7, 10],
            Self::Min7b5 => &[0, 3, 6, 10],
            Self::Dim7 => &[0, 3, 6, 9],
            Self::MinMaj7 => &[0, 3, 7, 11],
            Self::Maj6 => &[0, 4, 7, 9],
            Self::Min6 => &[0, 3, 7, 9],
            Self::Add9 => &[0, 4, 7, 14],
            Self::Power => &[0, 7],
        }
    }

    /// Display suffix appended to the root name ("Cm7", "F#sus4", "A5").
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Maj => "",
            Self::Min => "m",
            Self::Dim => "dim",
            Self::Aug => "aug",
            Self::Sus2 => "sus2",
            Self::Sus4 => "sus4",
            Self::Maj7 => "maj7",
            Self::Min7 => "m7",
            Self::Dom7 => "7",
            Self::Min7b5 => "m7b5",
            Self::Dim7 => "dim7",
            Self::MinMaj7 => "mMaj7",
            Self::Maj6 => "6",
            Self::Min6 => "m6",
            Self::Add9 => "add9",
            Self::Power => "5",
        }
    }
}

/// One chord on the session-global chord track. A chord lasts until the
/// next event's `start_tick` (the last chord lasts to the end of the song).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ChordEvent {
    pub start_tick: u64,
    pub root:       u8, // pitch class, 0=C .. 11=B
    pub quality:    ChordQuality,
}

impl ChordEvent {
    pub fn name(&self) -> String {
        format!("{}{}", NOTE_NAMES[(self.root % 12) as usize], self.quality.suffix())
    }

    /// Write this chord's MIDI pitches (root position around `octave`) into a
    /// caller-owned Vec — allocation-free on the audio thread once the Vec
    /// has grown to chord size, mirroring `generative::build_chord_into`.
    pub fn tones_into(&self, out: &mut Vec<u8>, octave: u8) {
        out.clear();
        let base = (octave as u32 * 12 + (self.root % 12) as u32).min(127) as u8;
        for &iv in self.quality.intervals() {
            out.push(base.saturating_add(iv).min(127));
        }
    }
}

/// Session-global chord track: an editable chord timeline plus the song key
/// (root + scale index into `generative::SCALE_NAMES`). When `events` is
/// non-empty it takes precedence over the legacy `HarmonyCenter` progression
/// for the per-block chord-tone broadcast; the key drives piano-roll scale
/// highlighting and per-track scale lock.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChordTrack {
    /// Sorted by `start_tick` (call `sort()` after edits).
    pub events:        Vec<ChordEvent>,
    pub key_root:      u8,    // 0=C .. 11=B
    pub key_scale_idx: usize, // index into generative::SCALE_NAMES
}

impl Default for ChordTrack {
    fn default() -> Self {
        Self { events: Vec::new(), key_root: 0, key_scale_idx: 0 }
    }
}

impl ChordTrack {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Chord sounding at `tick`: the last event with `start_tick <= tick`.
    pub fn chord_at(&self, tick: u64) -> Option<&ChordEvent> {
        match self.events.binary_search_by(|e| e.start_tick.cmp(&tick)) {
            Ok(i) => Some(&self.events[i]),
            Err(0) => None,
            Err(i) => Some(&self.events[i - 1]),
        }
    }

    pub fn sort(&mut self) {
        self.events.sort_by_key(|e| e.start_tick);
    }

    /// Key-scale intervals (semitones from `key_root`).
    pub fn key_intervals(&self) -> &'static [u8] {
        phonix_music::scales::SCALES
            .get(self.key_scale_idx)
            .copied()
            .unwrap_or(phonix_music::scales::MAJOR)
    }
}

// ── Conductor Track ───────────────────────────────────────────────────────────

/// One expression mark on the conductor timeline. `tempo_scale` ramps
/// linearly toward the NEXT event across the segment (ritardando /
/// accelerando); `dynamics` does the same (crescendo / diminuendo).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct ConductorEvent {
    pub start_tick:  u64,
    /// Dynamic level 0..1 (pp..ff); 0.5 = neutral (mf).
    pub dynamics:    f32,
    /// Local tempo scale 0.5..1.5; 1.0 = a tempo.
    pub tempo_scale: f32,
}

/// Session-global conductor: the "humanize everything" layer as a first-class
/// feature. Applies ONLY to sequenced playback (never to live input):
/// - breath rubato: the tempo breathes over `phrase_bars`,
/// - phrase-arc dynamics: velocities rise into mid-phrase, relax at the end,
/// - expression marks: cresc/dim + rit/accel ramps on the timeline,
/// - onset jitter + velocity scatter per note on MELODIC tracks
///   (deterministic hash of (track, tick, note): bounces are reproducible,
///   drums keep their machine timing).
/// Disabled by default — playback is bit-identical until switched on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ConductorTrack {
    pub enabled:         bool,
    /// Tempo breath depth, fraction of the tempo (0..0.1 = up to ±10%).
    pub rubato_depth:    f32,
    /// Breath / phrase-arc period in bars.
    pub phrase_bars:     f32,
    /// Phrase-arc velocity shaping depth 0..1.
    pub dynamics_depth:  f32,
    /// Per-note onset humanization on melodic tracks, ms (0..25).
    pub onset_jitter_ms: f32,
    /// Per-note velocity scatter 0..0.3.
    pub vel_scatter:     f32,
    /// Gesture entrances: notes that OPEN a phrase (first onset on a track
    /// after >= 1 beat of silence) enter softer and a touch later — the
    /// research toolkit's slide+swell gesture at the dispatch level. 0..1.
    #[serde(default = "ConductorTrack::default_entrance")]
    pub entrance_softness: f32,
    /// Expression marks, sorted by `start_tick`.
    pub events:          Vec<ConductorEvent>,
}

impl Default for ConductorTrack {
    fn default() -> Self {
        Self {
            enabled: false,
            rubato_depth: 0.02,
            phrase_bars: 4.0,
            dynamics_depth: 0.3,
            onset_jitter_ms: 6.0,
            vel_scatter: 0.08,
            entrance_softness: 0.25,
            events: Vec::new(),
        }
    }
}

impl ConductorTrack {
    fn default_entrance() -> f32 { 0.25 }

    fn phrase_ticks(&self) -> f64 {
        (self.phrase_bars.max(0.25) as f64) * 4.0 * PPQN as f64
    }

    /// Interpolated expression-mark values at `tick`:
    /// (tempo_scale, dynamics). Neutral (1.0, 0.5) with no events.
    fn event_values(&self, tick: u64) -> (f32, f32) {
        if self.events.is_empty() {
            return (1.0, 0.5);
        }
        let idx = match self.events.binary_search_by(|e| e.start_tick.cmp(&tick)) {
            Ok(i) => i as isize,
            Err(i) => i as isize - 1,
        };
        if idx < 0 {
            return (1.0, 0.5); // before the first mark: a tempo, mf
        }
        let cur = &self.events[idx as usize];
        match self.events.get(idx as usize + 1) {
            Some(next) if next.start_tick > cur.start_tick => {
                let t = (tick - cur.start_tick) as f32
                    / (next.start_tick - cur.start_tick) as f32;
                (
                    cur.tempo_scale + (next.tempo_scale - cur.tempo_scale) * t,
                    cur.dynamics + (next.dynamics - cur.dynamics) * t,
                )
            }
            _ => (cur.tempo_scale, cur.dynamics),
        }
    }

    /// Tempo multiplier at `tick` (1.0 when disabled).
    pub fn tempo_mult(&self, tick: u64) -> f32 {
        if !self.enabled {
            return 1.0;
        }
        let (scale, _) = self.event_values(tick);
        let pos = (tick as f64 % self.phrase_ticks()) / self.phrase_ticks();
        let breath = (std::f64::consts::TAU * pos).sin() as f32;
        (scale * (1.0 + self.rubato_depth.clamp(0.0, 0.2) * breath)).clamp(0.25, 2.0)
    }

    /// Velocity multiplier at `tick` (1.0 when disabled).
    pub fn dyn_mult(&self, tick: u64) -> f32 {
        if !self.enabled {
            return 1.0;
        }
        let (_, dynamics) = self.event_values(tick);
        // marks: 0..1 -> x0.55..x1.45 around mf
        let base = 0.55 + 0.9 * dynamics.clamp(0.0, 1.0);
        // phrase arc: rise into mid-phrase, relax at the end
        let pos = (tick as f64 % self.phrase_ticks()) / self.phrase_ticks();
        let arc = (std::f64::consts::PI * pos).sin() as f32 - 0.5;
        (base * (1.0 + self.dynamics_depth.clamp(0.0, 1.0) * 0.6 * arc)).clamp(0.2, 2.0)
    }

    pub fn sort(&mut self) {
        self.events.sort_by_key(|e| e.start_tick);
    }
}

// ── Session ───────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Session {
    pub name:     String,
    pub bpm:      f32,
    pub time_sig: (u8, u8),
    pub tracks:   Vec<TrackConfig>,
    pub patterns: Vec<Pattern>,   // global pool; referenced by id
    pub mixer:    MixerState,
    pub next_pat_id: usize,
    /// Loop region — persisted so reopening a session restores the loop markers.
    #[serde(default)]
    pub loop_active:    bool,
    #[serde(default = "Session::default_loop_start_bar")]
    pub loop_start_bar: u32,  // 1-based
    #[serde(default = "Session::default_loop_end_bar")]
    pub loop_end_bar:   u32,  // 1-based, exclusive
    /// Optional session-wide harmonic center (chord progression shared by all tracks).
    #[serde(default)]
    pub harmony_center: Option<HarmonyCenter>,
    /// Global modulation matrix: LFOs/envelopes/macros routed to any parameter.
    #[serde(default)]
    pub mod_matrix: crate::mod_matrix::ModMatrix,
    /// Session-global chord track (editable chord timeline + song key).
    /// Empty events = feature off; `harmony_center` then applies as before.
    #[serde(default)]
    pub chord_track: ChordTrack,
    /// Session-global conductor (rubato / phrase dynamics / humanization).
    /// Disabled by default: playback stays bit-identical until enabled.
    #[serde(default)]
    pub conductor: ConductorTrack,
    /// Last groove template extracted from an audio clip (micro-timing feel),
    /// available to apply as swing from the piano roll. None until extracted.
    #[serde(default)]
    pub groove: Option<phonix_music::groove::GrooveTemplate>,
    /// Arranger sections (Intro/Verse/Chorus …), sorted by start_tick.
    #[serde(default)]
    pub sections: Vec<Section>,
    /// Tempo after tick zero; `bpm` holds until the first point.
    #[serde(default)]
    pub tempo_map: Vec<crate::tempo::TempoPoint>,
    /// Meter after tick zero; `time_sig` holds until the first change.
    #[serde(default)]
    pub meter_map: Vec<crate::tempo::MeterChange>,
    /// Plugin delay compensation switched off: tracks and buses play as
    /// early as their plugins allow, unaligned.
    #[serde(default)]
    pub pdc_bypass: bool,
    /// The clocks the transport sends and follows.
    #[serde(default)]
    pub sync: SyncSettings,
    /// Named positions along the arrangement, sorted by tick.
    #[serde(default)]
    pub markers: Vec<Marker>,
    /// Recording only lands between the punch bars while this is on.
    #[serde(default)]
    pub punch_active: bool,
    /// Punch range in bars, 1-based, the end exclusive.
    #[serde(default = "Session::default_punch_in")]
    pub punch_in_bar: u32,
    #[serde(default = "Session::default_punch_out")]
    pub punch_out_bar: u32,
    /// Bars of metronome before a recording pass starts; zero for none.
    #[serde(default)]
    pub count_in_bars: u8,
    /// Folders grouping tracks in the lists; a track names its folder.
    #[serde(default)]
    pub folders: Vec<Folder>,
    /// The session view's scenes by slot; a slot past the list is a plain one.
    #[serde(default)]
    pub scenes: Vec<Scene>,
    /// The video laid along the arrangement, at most one.
    #[serde(default)]
    pub video: Option<VideoClip>,
    /// The tuning every in-house instrument plays in.
    #[serde(default)]
    pub tuning: Tuning,
    /// The groove pool: every feel extracted so far, by name.
    #[serde(default)]
    pub grooves: Vec<NamedGroove>,
    /// Retune the in-house instruments to the chord track's chord as it
    /// plays: just intonation above the chord root, the root left alone.
    #[serde(default)]
    pub adaptive_intonation: bool,
}

/// A groove in the pool: the feel and the clip it came from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NamedGroove {
    pub name: String,
    pub template: phonix_music::groove::GrooveTemplate,
}

/// A tuning: the scale's degrees in cents above the root, the last one
/// being the period, one key per degree from the root; the root key; and
/// the frequency of A4, which anchors the whole.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tuning {
    pub name: String,
    pub steps: Vec<f64>,
    pub root: u8,
    pub ref_hz: f64,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning::equal("12-TET", 12)
    }
}

/// The MIDI note of A4, which the reference frequency is given for.
pub const REF_NOTE: u8 = 69;
/// The MIDI note of middle C, the root of every preset.
pub const ROOT_NOTE: u8 = 60;
/// The reference frequency of A4 in the presets.
pub const REF_HZ: f64 = 440.0;

fn cents(ratio: f64) -> f64 {
    1200.0 * ratio.log2()
}

impl Tuning {
    /// `n` equal steps to the octave.
    pub fn equal(name: &str, n: usize) -> Self {
        Tuning {
            name: name.into(),
            steps: (1..=n).map(|i| i as f64 * 1200.0 / n as f64).collect(),
            root: ROOT_NOTE,
            ref_hz: REF_HZ,
        }
    }

    fn from_ratios(name: &str, ratios: &[(u32, u32)]) -> Self {
        Tuning {
            name: name.into(),
            steps: ratios.iter().map(|&(a, b)| cents(a as f64 / b as f64)).collect(),
            root: ROOT_NOTE,
            ref_hz: REF_HZ,
        }
    }

    fn from_cents(name: &str, steps: &[f64]) -> Self {
        Tuning { name: name.into(), steps: steps.to_vec(), root: ROOT_NOTE, ref_hz: REF_HZ }
    }

    /// The tunings offered by name.
    pub fn presets() -> Vec<Tuning> {
        vec![
            Tuning::default(),
            Tuning::from_ratios("Pythagorean", &[
                (2187, 2048), (9, 8), (32, 27), (81, 64), (4, 3), (729, 512),
                (3, 2), (6561, 4096), (27, 16), (16, 9), (243, 128), (2, 1),
            ]),
            Tuning::from_ratios("Just intonation", &[
                (16, 15), (9, 8), (6, 5), (5, 4), (4, 3), (45, 32),
                (3, 2), (8, 5), (5, 3), (9, 5), (15, 8), (2, 1),
            ]),
            Tuning::from_cents("Quarter-comma meantone", &[
                76.049, 193.157, 310.265, 386.314, 503.422, 579.471,
                696.578, 772.627, 889.735, 1006.843, 1082.892, 1200.0,
            ]),
            Tuning::from_cents("Werckmeister III", &[
                90.225, 192.180, 294.135, 390.225, 498.045, 588.270,
                696.090, 792.180, 888.270, 996.090, 1092.180, 1200.0,
            ]),
            Tuning::equal("19-TET", 19),
            Tuning::equal("24-TET", 24),
            Tuning::equal("31-TET", 31),
        ]
    }

    /// A tuning read from a Scala scale file: a description line, the
    /// degree count, then one pitch per line as cents (with a point) or a
    /// ratio; comment lines start with an exclamation mark.
    pub fn from_scala(text: &str, name: &str) -> Result<Tuning, String> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.starts_with('!'));
        let description = lines.next().ok_or("empty file")?;
        let count: usize = lines.next().ok_or("no degree count")?
            .split_whitespace().next().unwrap_or("").parse().map_err(|_| "bad degree count")?;
        let mut steps = Vec::with_capacity(count);
        for line in lines {
            let Some(tok) = line.split_whitespace().next() else { continue };
            let v = if tok.contains('.') {
                tok.parse::<f64>().map_err(|_| format!("bad cents: {tok}"))?
            } else if let Some((a, b)) = tok.split_once('/') {
                let (a, b): (f64, f64) = (a.parse().map_err(|_| format!("bad ratio: {tok}"))?, b.parse().map_err(|_| format!("bad ratio: {tok}"))?);
                if b <= 0.0 || a <= 0.0 { return Err(format!("bad ratio: {tok}")); }
                cents(a / b)
            } else {
                let a: f64 = tok.parse().map_err(|_| format!("bad pitch: {tok}"))?;
                if a <= 0.0 { return Err(format!("bad pitch: {tok}")); }
                cents(a)
            };
            steps.push(v);
            if steps.len() == count { break; }
        }
        if steps.len() != count || count == 0 {
            return Err(format!("{count} degrees announced, {} read", steps.len()));
        }
        let name = if name.is_empty() { description.to_string() } else { name.to_string() };
        Ok(Tuning { name, steps, root: ROOT_NOTE, ref_hz: REF_HZ })
    }

    /// Cents of a note above the root, one key per degree, the period
    /// repeating past the last degree.
    pub fn cents_of(&self, note: u8) -> f64 {
        let n = self.steps.len();
        let d = note as i64 - self.root as i64;
        if n == 0 {
            return d as f64 * 100.0;
        }
        let period = self.steps[n - 1];
        let oct = d.div_euclid(n as i64) as f64;
        let deg = d.rem_euclid(n as i64) as usize;
        oct * period + if deg == 0 { 0.0 } else { self.steps[deg - 1] }
    }

    /// The frequency of a note, with A4 at the reference frequency.
    pub fn hz_of(&self, note: u8) -> f64 {
        self.ref_hz * 2f64.powf((self.cents_of(note) - self.cents_of(REF_NOTE)) / 1200.0)
    }
}

#[cfg(test)]
mod tuning_tests {
    use super::*;

    #[test]
    fn equal_temperament_is_the_default() {
        let t = Tuning::default();
        assert!((t.hz_of(69) - 440.0).abs() < 1e-9);
        assert!((t.hz_of(60) - 261.6256).abs() < 1e-3);
        assert!((t.hz_of(81) - 880.0).abs() < 1e-9);
    }

    #[test]
    fn just_intonation_holds_its_ratios_from_the_root() {
        let t = Tuning::presets().into_iter().find(|t| t.name == "Just intonation").unwrap();
        let c = t.hz_of(60);
        assert!((t.hz_of(64) / c - 1.25).abs() < 1e-9, "major third");
        assert!((t.hz_of(67) / c - 1.5).abs() < 1e-9, "fifth");
        assert!((t.hz_of(72) / c - 2.0).abs() < 1e-9, "octave");
        assert!((t.hz_of(48) / c - 0.5).abs() < 1e-9, "octave below");
        assert!((t.hz_of(69) - 440.0).abs() < 1e-9, "A4 is anchored");
    }

    #[test]
    fn nineteen_steps_span_an_octave_and_the_reference_scales() {
        let mut t = Tuning::equal("19-TET", 19);
        assert!((t.hz_of(79) / t.hz_of(60) - 2.0).abs() < 1e-9);
        t.ref_hz = 432.0;
        assert!((t.hz_of(69) - 432.0).abs() < 1e-9);
    }

    #[test]
    fn a_scala_file_reads_cents_and_ratios() {
        let text = "! test.scl
!
A five note scale
 5
!
 9/8
 400.0
 3/2
 1000.0
 2
";
        let t = Tuning::from_scala(text, "").unwrap();
        assert_eq!(t.name, "A five note scale");
        assert_eq!(t.steps.len(), 5);
        assert!((t.steps[0] - 203.91).abs() < 0.01);
        assert!((t.steps[1] - 400.0).abs() < 1e-9);
        assert!((t.steps[4] - 1200.0).abs() < 1e-9);
        assert!((t.hz_of(65) / t.hz_of(60) - 2.0).abs() < 1e-9, "five keys make the period");
        assert!(Tuning::from_scala("desc
 3
 100.0
", "").is_err());
    }
}

/// A video laid along the arrangement: the file, the tick its first shown
/// frame plays at, and how far into the file that frame is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VideoClip {
    pub path: String,
    #[serde(default)]
    pub start_tick: u64,
    #[serde(default)]
    pub offset_secs: f64,
}

/// A folder of tracks: drawn as a header before its first member, its
/// members hidden while collapsed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    #[serde(default)]
    pub color: [u8; 3],
    #[serde(default)]
    pub collapsed: bool,
}

impl Session {
    /// The folder whose first member in track order is `track`.
    pub fn folder_starting_at(&self, track: usize) -> Option<usize> {
        let f = self.tracks.get(track)?.folder? as usize;
        if f >= self.folders.len() { return None; }
        let first = self.tracks.iter().position(|t| t.folder == Some(f as u8))?;
        (first == track).then_some(f)
    }

    /// Whether a track sits in a collapsed folder.
    pub fn track_hidden(&self, track: usize) -> bool {
        self.tracks.get(track)
            .and_then(|t| t.folder)
            .and_then(|f| self.folders.get(f as usize))
            .map(|f| f.collapsed)
            .unwrap_or(false)
    }
}

/// A named position along the arrangement.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Marker {
    pub tick: u64,
    pub name: String,
}

impl Session {
    fn default_loop_start_bar() -> u32 { 1 }
    fn default_loop_end_bar()   -> u32 { 5 }
}

impl Default for Session {
    fn default() -> Self {
        let mut s = Self {
            name:        "New Session".to_string(),
            bpm:         120.0,
            time_sig:    (4, 4),
            tracks:      Vec::new(),
            patterns:    Vec::new(),
            mixer:       MixerState::default(),
            next_pat_id: 0,
            loop_active:    false,
            loop_start_bar: 1,
            loop_end_bar:   5,
            harmony_center: None,
            mod_matrix:     Default::default(),
            chord_track:    Default::default(),
            conductor:      Default::default(),
            groove:         None,
            sections:       Vec::new(),
            tempo_map:      Vec::new(),
            meter_map:      Vec::new(),
            pdc_bypass:     false,
            sync:           Default::default(),
            markers:        Vec::new(),
            punch_active:   false,
            punch_in_bar:   1,
            punch_out_bar:  2,
            count_in_bars:  0,
            folders:        Vec::new(),
            scenes:         Vec::new(),
            video:          None,
            tuning:         Default::default(),
            grooves:        Vec::new(),
            adaptive_intonation: false,
        };
        // Default: one melodic track + one Drum track
        s.add_track(TrackConfig {
            name:  "Lead".to_string(),
            kind:  TrackKind::Melodic,
            engine: TrackSource::Empty,
            color: [80, 160, 220],
            ..Default::default()
        });
        s.add_track(TrackConfig {
            name:  "Drums".to_string(),
            kind:  TrackKind::Drum,
            engine: TrackSource::Empty,
            color: [220, 80, 80],
            ..Default::default()
        });
        // Create initial patterns for each track
        let lead_pat = s.new_piano_roll_pattern("Pattern 1", 2);
        s.tracks[0].session_clips[0] = Some(lead_pat);
        s.tracks[0].timeline.push(ClipPlacement { pattern_id: lead_pat, start_tick: 0, length_tick: 0 , ..Default::default()});

        let drum_pat = s.new_drum_pattern("Pattern 1", 1, default_drum_rows());
        s.tracks[1].session_clips[0] = Some(drum_pat);
        s.tracks[1].timeline.push(ClipPlacement { pattern_id: drum_pat, start_tick: 0, length_tick: 0 , ..Default::default()});
        s
    }
}

impl Session {
    fn alloc_id(&mut self) -> usize {
        let id = self.next_pat_id;
        self.next_pat_id += 1;
        id
    }

    /// Public: allocate a new pattern id (used for manual pattern construction).
    pub fn alloc_pattern_id(&mut self) -> usize { self.alloc_id() }

    /// After removing the track at `deleted`, fix every other track's
    /// index-based references (audio_input routing source, sidechain source):
    /// a reference to the removed track is cleared, and any higher index is
    /// shifted down by one to track the Vec::remove. Without this, deleting a
    /// track leaves routes/sidechains pointing at the wrong (or a dangling)
    /// track. Call immediately after `self.tracks.remove(deleted)`.
    pub fn remap_track_refs_after_delete(&mut self, deleted: usize) {
        let fix = |r: &mut Option<u16>| {
            if let Some(src) = *r {
                let src = src as usize;
                if src == deleted { *r = None; }
                else if src > deleted { *r = Some((src - 1) as u16); }
            }
        };
        for t in &mut self.tracks {
            fix(&mut t.audio_input);
            for slot in &mut t.fx.slots {
                let mut src = sidechain_track(slot);
                fix(&mut src);
                set_sidechain_track(slot, src);
            }
        }
    }

    /// Inverse of `remap_track_refs_after_delete`: after inserting a track at
    /// `inserted` (e.g. undoing a delete), shift every index-based reference at
    /// or above `inserted` up by one so routes/sidechains keep pointing at the
    /// same tracks. Call immediately after `self.tracks.insert(inserted, ..)`.
    /// After swapping the tracks at `a` and `b` (e.g. a move up/down), swap
    /// every index-based reference too: a reference to `a` now points at `b`
    /// and vice versa, so routes/sidechains follow the tracks they named.
    pub fn remap_track_refs_after_swap(&mut self, a: usize, b: usize) {
        let (a, b) = (a as u16, b as u16);
        let swap = |r: &mut Option<u16>| {
            if *r == Some(a) { *r = Some(b); }
            else if *r == Some(b) { *r = Some(a); }
        };
        for t in &mut self.tracks {
            swap(&mut t.audio_input);
            for slot in &mut t.fx.slots {
                let mut src = sidechain_track(slot);
                swap(&mut src);
                set_sidechain_track(slot, src);
            }
        }
    }

    pub fn remap_track_refs_after_insert(&mut self, inserted: usize) {
        let bump = |r: &mut Option<u16>| {
            if let Some(src) = *r {
                if (src as usize) >= inserted { *r = Some(src + 1); }
            }
        };
        for (i, t) in self.tracks.iter_mut().enumerate() {
            if i == inserted { continue; } // the re-inserted track keeps its own refs
            bump(&mut t.audio_input);
            for slot in &mut t.fx.slots {
                let mut src = sidechain_track(slot);
                bump(&mut src);
                set_sidechain_track(slot, src);
            }
        }
    }

    /// Moves every chain written in the old form into its named field and
    /// clears the old one, so the next save carries no trace of it. A chain
    /// already named is left alone.
    pub fn take_legacy_fx(&mut self) -> phonix_legacy::Report {
        let mut report = phonix_legacy::Report::default();
        let mut take = |old: &mut Vec<phonix_legacy::FxSlotState>, new: &mut ChainSpec| {
            if old.is_empty() {
                return;
            }
            let (chain, r) = phonix_legacy::convert_chain(old);
            report.unknown_kinds.extend(r.unknown_kinds);
            report.unconverted.extend(r.unconverted);
            if new.is_empty() {
                *new = chain;
            }
            old.clear();
        };
        for t in &mut self.tracks {
            take(&mut t.fx_slots, &mut t.fx);
        }
        take(&mut self.mixer.master_fx_slots, &mut self.mixer.master_fx);
        take(&mut self.mixer.mix_fx_slots, &mut self.mixer.mix_fx);
        for g in &mut self.mixer.group_buses {
            take(&mut g.fx_slots, &mut g.fx);
        }
        report
    }

    pub fn add_track(&mut self, cfg: TrackConfig) {
        self.tracks.push(cfg);
    }

    pub fn new_piano_roll_pattern(&mut self, name: &str, bars: u32) -> usize {
        let id = self.alloc_id();
        self.patterns.push(Pattern::new_piano_roll(id, name, bars));
        id
    }

    pub fn new_drum_pattern(&mut self, name: &str, bars: u32, rows: Vec<StepRow>) -> usize {
        let id = self.alloc_id();
        self.patterns.push(Pattern::new_steps(id, name, bars, 16, rows));
        id
    }

    /// Ticks in one bar at this session's time signature. The unit the
    /// arrangement grid, clip lengths and section markers are all counted in.
    pub fn bar_ticks(&self) -> u64 {
        bar_ticks_of(self.time_sig)
    }

    /// Length of a pattern in ticks, at this session's metre. 0 when the
    /// pattern is gone, which is what every caller already treated as "no
    /// content".
    pub fn pattern_len_ticks(&self, id: usize) -> u64 {
        self.pattern_by_id(id).map(|p| p.length_ticks(self.bar_ticks())).unwrap_or(0)
    }

    /// How long a placement occupies on the timeline: its explicit length, or
    /// the pattern's natural one.
    pub fn placement_len_ticks(&self, placement: &ClipPlacement) -> u64 {
        if placement.length_tick > 0 {
            placement.length_tick
        } else {
            self.pattern_len_ticks(placement.pattern_id)
        }
    }

    pub fn pattern_by_id(&self, id: usize) -> Option<&Pattern> {
        self.patterns.iter().find(|p| p.id == id)
    }

    /// Holds the returns to the most allowed and gives every track one
    /// send per return, new ones at zero.
    pub fn fit_sends(&mut self) {
        self.mixer.send_returns.truncate(MAX_SENDS);
        let n = self.mixer.send_returns.len();
        for t in &mut self.tracks {
            t.sends.resize(n, 0.0);
        }
    }

    pub fn pattern_by_id_mut(&mut self, id: usize) -> Option<&mut Pattern> {
        self.patterns.iter_mut().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod artic_serde_tests {
    use super::*;
    #[test]
    fn artic_lane_round_trips_through_json() {
        let mut p = Pattern::new_piano_roll(1, "g", 2);
        p.artic = vec![
            ArticPoint { tick: 0,   event: ArticEvent::SlideTime(0.085) },
            ArticPoint { tick: 120, event: ArticEvent::PowerChord(true) },
            ArticPoint { tick: 240, event: ArticEvent::PitchBend(-1.5) },
            ArticPoint { tick: 360, event: ArticEvent::SlapPop(0.7) },
        ];
        let j = serde_json::to_string(&p).unwrap();
        let back: Pattern = serde_json::from_str(&j).unwrap();
        assert_eq!(back.artic.len(), 4);
        assert_eq!(back.artic[1].event, ArticEvent::PowerChord(true));
        // old patterns with no artic field still deserialize (serde default)
        let legacy = r#"{"id":2,"name":"x","length_bars":1,"data":{"PianoRoll":[]}}"#;
        let lp: Pattern = serde_json::from_str(legacy).unwrap();
        assert!(lp.artic.is_empty());
    }
}

#[cfg(test)]
mod chord_track_tests {
    use super::*;

    #[test]
    fn chord_at_picks_last_event_at_or_before_tick() {
        let mut ct = ChordTrack::default();
        let bar = PPQN as u64 * 4;
        ct.events = vec![
            ChordEvent { start_tick: bar,     root: 2, quality: ChordQuality::Min7 },
            ChordEvent { start_tick: 3 * bar, root: 7, quality: ChordQuality::Maj },
        ];
        assert!(ct.chord_at(0).is_none(), "before the first chord = no chord");
        assert_eq!(ct.chord_at(bar).unwrap().root, 2, "exact boundary");
        assert_eq!(ct.chord_at(2 * bar).unwrap().root, 2, "inside the span");
        assert_eq!(ct.chord_at(3 * bar).unwrap().root, 7);
        assert_eq!(ct.chord_at(u64::MAX).unwrap().root, 7, "last chord lasts forever");
    }

    #[test]
    fn chord_event_tones_root_position() {
        let ev = ChordEvent { start_tick: 0, root: 2, quality: ChordQuality::Min7 }; // Dm7
        let mut out = Vec::new();
        ev.tones_into(&mut out, 4);
        assert_eq!(out, vec![50, 53, 57, 60]);
        assert_eq!(ev.name(), "Dm7");
    }

    /// Sessions saved before the chord track existed must load with the
    /// default (empty) chord track — same serde(default) tolerance the
    /// artic/probability fields rely on.
    #[test]
    fn session_without_chord_track_gets_default() {
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v.as_object_mut().unwrap().remove("chord_track");
        let s: Session = serde_json::from_value(v).unwrap();
        assert!(s.chord_track.is_empty());
        assert_eq!(s.chord_track.key_scale_idx, 0);
    }

    #[test]
    fn chord_track_roundtrips_in_session() {
        let mut s = Session::default();
        s.chord_track.key_root = 2;
        s.chord_track.key_scale_idx = 1;
        s.chord_track.events.push(ChordEvent {
            start_tick: 960, root: 9, quality: ChordQuality::Sus4,
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.chord_track, s.chord_track);
    }
}

#[cfg(test)]
mod conductor_tests {
    use super::*;

    #[test]
    fn disabled_conductor_is_strictly_neutral() {
        let c = ConductorTrack::default();
        assert!(!c.enabled);
        for tick in [0u64, 480, 12_345, 1_000_000] {
            assert_eq!(c.tempo_mult(tick), 1.0);
            assert_eq!(c.dyn_mult(tick), 1.0);
        }
    }

    #[test]
    fn expression_marks_interpolate_linearly() {
        let bar = PPQN as u64 * 4;
        let mut c = ConductorTrack {
            enabled: true,
            rubato_depth: 0.0,
            dynamics_depth: 0.0,
            ..Default::default()
        };
        c.events = vec![
            ConductorEvent { start_tick: 0, dynamics: 0.5, tempo_scale: 1.0 },
            ConductorEvent { start_tick: 4 * bar, dynamics: 1.0, tempo_scale: 0.5 },
        ];
        // Midpoint of the ritardando: halfway between the marks.
        let mid = c.tempo_mult(2 * bar);
        assert!((mid - 0.75).abs() < 1e-3, "ritardando midpoint {mid}");
        // After the last mark: hold its values.
        assert!((c.tempo_mult(10 * bar) - 0.5).abs() < 1e-3);
        // Dynamics ramp: crescendo midpoint 0.75 -> mult 0.55 + 0.9*0.75.
        let dm = c.dyn_mult(2 * bar);
        assert!((dm - (0.55 + 0.9 * 0.75)).abs() < 1e-2, "crescendo midpoint {dm}");
        // Before the first mark with none earlier: neutral.
        c.events.remove(0);
        assert_eq!(c.tempo_mult(bar), 1.0);
    }

    #[test]
    fn rubato_breathes_around_the_nominal_tempo() {
        let c = ConductorTrack {
            enabled: true,
            rubato_depth: 0.05,
            phrase_bars: 4.0,
            ..Default::default()
        };
        let phrase = (4.0 * 4.0 * PPQN as f64) as u64;
        let up = c.tempo_mult(phrase / 4);   // sin peak
        let down = c.tempo_mult(3 * phrase / 4);
        assert!(up > 1.04 && up < 1.06, "breath peak {up}");
        assert!(down < 0.96 && down > 0.94, "breath trough {down}");
        assert!((c.tempo_mult(0) - 1.0).abs() < 1e-3, "phrase start is a tempo");
    }

    #[test]
    fn session_without_conductor_gets_default() {
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v.as_object_mut().unwrap().remove("conductor");
        let s: Session = serde_json::from_value(v).unwrap();
        assert!(!s.conductor.enabled);
    }
}

#[cfg(test)]
mod stretch_sync_tests {
    use super::*;

    #[test]
    fn effective_stretch_prefers_tempo_sync() {
        let mut pl = ClipPlacement::default();
        assert_eq!(pl.effective_stretch(120.0), 1.0, "defaults are native speed");
        pl.stretch = 1.5;
        assert!((pl.effective_stretch(120.0) - 1.5).abs() < 1e-6, "manual ratio");
        pl.source_bpm = 140.0;
        assert!((pl.effective_stretch(70.0) - 2.0).abs() < 1e-6,
            "tempo-sync overrides manual: 140 -> 70 bpm doubles the clip");
        assert!((pl.effective_stretch(140.0) - 1.0).abs() < 1e-6, "same tempo = native");
        // Extreme tempo gap clamps to the WSOLA-safe range.
        assert!((pl.effective_stretch(10.0) - 4.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod legacy_fx {
    use super::*;

    #[test]
    fn a_session_written_with_ordinals_opens_named_and_saves_without_them() {
        let mut v = serde_json::to_value(Session::default()).unwrap();
        v["tracks"] = serde_json::json!([serde_json::to_value(TrackConfig::default()).unwrap()]);
        v["tracks"][0]["fx_slots"] = serde_json::json!([
            { "effect_type": 20, "enabled": true, "mix": 1.0, "params": [] },
            { "effect_type": 0, "enabled": false, "mix": 1.0, "params": [] },
            { "effect_type": 8, "enabled": true, "mix": 0.7, "params": [], "sidechain_src": 2 }
        ]);
        v["mixer"]["master_fx_slots"] = serde_json::json!([
            { "effect_type": 28, "enabled": true, "mix": 1.0, "params": [] }
        ]);
        let mut s: Session = serde_json::from_value(v).unwrap();
        assert!(s.tracks[0].fx.is_empty());
        let report = s.take_legacy_fx();
        assert!(report.is_clean(), "{report:?}");
        assert_eq!(s.tracks[0].fx.kinds().collect::<Vec<_>>(), vec!["parametric-eq", "compressor"]);
        assert_eq!(sidechain_track(&s.tracks[0].fx.slots[1]), Some(2));
        assert_eq!(s.mixer.master_fx.kinds().collect::<Vec<_>>(), vec!["brickwall-limiter"]);
        assert!(s.tracks[0].fx_slots.is_empty() && s.mixer.master_fx_slots.is_empty());
        let text = serde_json::to_string(&s).unwrap();
        for key in ["\"fx_slots\"", "\"master_fx_slots\"", "\"mix_fx_slots\""] {
            assert!(!text.contains(key), "{key} is written again");
        }
        assert!(text.contains("\"parametric-eq\""));
    }

    #[test]
    fn track_references_follow_a_keyed_slot_through_delete_swap_and_insert() {
        let mut s = Session::default();
        s.tracks = (0..4).map(|_| TrackConfig::default()).collect();
        let mut slot = SlotSpec::new("compressor");
        set_sidechain_track(&mut slot, Some(3));
        s.tracks[0].fx = ChainSpec::new(vec![slot]);
        s.tracks.remove(1);
        s.remap_track_refs_after_delete(1);
        assert_eq!(sidechain_track(&s.tracks[0].fx.slots[0]), Some(2));
        s.remap_track_refs_after_swap(0, 2);
        assert_eq!(sidechain_track(&s.tracks[0].fx.slots[0]), Some(0));
        s.tracks.insert(0, TrackConfig::default());
        s.remap_track_refs_after_insert(0);
        assert_eq!(sidechain_track(&s.tracks[1].fx.slots[0]), Some(1));
    }
}
