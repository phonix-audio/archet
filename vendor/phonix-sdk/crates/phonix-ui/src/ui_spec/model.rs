//! Declarative plugin-UI layout spec (serde, loaded from a `.ron` file).
//!
//! BOUNDARY: this describes LAYOUT only. Every `param:` must name a field that
//! already exists on the plugin's `Patch` struct; a spec relocates / relabels /
//! re-curves existing parameters, it cannot invent a new one (that still needs a
//! Rust field + engine DSP + recompile).

use serde::{Deserialize, Serialize};

pub use crate::widgets::{Curve, Unit};

/// A whole plugin editor's body, as data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginUiSpec {
    pub title: String,
    /// Plugin accent colour, `[r, g, b]`.
    pub accent: [u8; 3],
    /// Column count for a single-tab (untabbed) body. Sections place themselves
    /// with `SectionSpec::column`. Ignored when `tabs.len() > 1`.
    #[serde(default)]
    pub columns: Option<usize>,
    /// Knob diameter for this editor's body, in points. Defaults to
    /// `KNOB_SMALL` (32).
    ///
    /// This exists so converting a hand-written editor doesn't silently shrink
    /// its knobs: several editors drew at `KNOB_MEDIUM` (48), and a renderer
    /// that hardcoded one size made every conversion a quiet visual downgrade.
    /// It is deliberately spec-wide rather than per-control — a plugin picks ONE
    /// knob size, and letting individual rows differ is exactly the drift the
    /// shared renderer exists to prevent.
    #[serde(default)]
    pub knob_size: Option<f32>,
    pub tabs: Vec<TabSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TabSpec {
    pub label: String,
    /// Column count for this tab's grid (defaults to 3 or the section max+1).
    #[serde(default)]
    pub columns: Option<usize>,
    /// RELATIVE column widths, e.g. `[1.5, 1.2, 1.0, 1.3]`. Empty = equal
    /// columns.
    ///
    /// Several hand-written editors lay their columns out in unequal
    /// proportions (a shared row of 20/30/25/25). Equal columns are the
    /// common case, but forcing them on those editors would be a visible
    /// downgrade, so the grid can be told the proportions instead.
    #[serde(default)]
    pub column_weights: Vec<f32>,
    #[serde(default)]
    pub sections: Vec<SectionSpec>,
    /// Stacked BANDS of columns, when one grid is not enough.
    ///
    /// A layer tab of four equal panels over the top 55% of the height,
    /// then a separate 20/30/25/25 row underneath. A single column grid cannot
    /// say that, and splitting it into two tabs would change how the editor is
    /// navigated, which is a loss. When this is non-empty it REPLACES `sections`
    /// / `columns` / `column_weights`, which stay as the one-band shorthand
    /// every other spec uses.
    #[serde(default)]
    pub bands: Vec<BandSpec>,
}

/// One horizontal band of columns inside a tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BandSpec {
    #[serde(default)]
    pub columns: Option<usize>,
    #[serde(default)]
    pub column_weights: Vec<f32>,
    /// Fraction of the tab's height this band takes. `None` = whatever it
    /// needs; the LAST band without one absorbs the remainder.
    #[serde(default)]
    pub height_frac: Option<f32>,
    /// Draw a separator under this band.
    #[serde(default)]
    pub separator_below: bool,
    pub sections: Vec<SectionSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionSpec {
    pub title: String,
    /// Overrides the plugin accent for this section's header.
    #[serde(default)]
    pub accent: Option<[u8; 3]>,
    /// Which grid column this section lives in (0-based).
    #[serde(default)]
    pub column: Option<usize>,
    /// LAYOUT-ONLY mode: a section id drawn by the editor's own hand-written
    /// method (via `render_layout`'s callback), which also draws its header. When
    /// set, `rows` is ignored. Used by typed-command plugins that
    /// keep their sections in Rust but make tab/column PLACEMENT declarative.
    #[serde(default)]
    pub section: Option<String>,
    #[serde(default)]
    pub rows: Vec<RowSpec>,
    /// Grey this section out when the condition is false.
    #[serde(default)]
    pub enabled_when: Option<EnableCond>,
    /// Icon drawn in the section header, by name (see `gui::icons::by_name`).
    #[serde(default)]
    pub icon: Option<String>,
    /// A BOOL patch field toggled from the section header.
    ///
    /// Several editors put a layer's on/off switch in its header, where the
    /// icon and title also carry the state by dimming. That is a genuinely
    /// better control than a checkbox in the body, so the spec can express it
    /// rather than conversions having to give it up.
    #[serde(default)]
    pub header_toggle: Option<String>,
    /// Fixed maximum width for this section, in points. For editors that size
    /// their panels rather than letting a grid divide the space.
    #[serde(default)]
    pub max_width: Option<f32>,
}

/// "This control is only meaningful when `param` holds one of `equals`."
///
/// It GREYS OUT, it never hides. Showing and hiding controls as a mode changes
/// makes the surrounding layout jump under the user's cursor, which is why the
/// house rule is a fixed layout with `add_enabled_ui`. Expressing the condition
/// here rather than in each editor is what makes the rule structural: a spec
/// simply has no way to say "hide".
///
/// `param` may name an enum field (matched by variant name), an int field
/// (matched by its decimal form) or a bool field (`"true"` / `"false"`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnableCond {
    pub param: String,
    pub equals: Vec<String>,
    /// Invert the match ("enabled when NOT one of these").
    #[serde(default)]
    pub not: bool,
}

/// A horizontal row of controls.
pub type RowSpec = Vec<ControlSpec>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlSpec {
    /// Display label (knob caption, button text, dropdown prefix).
    #[serde(default)]
    pub label: String,
    /// Patch field name this control reads/writes. Empty for `Custom`/`Label`.
    #[serde(default)]
    pub param: String,
    pub kind: ControlKind,
    /// Fixed cell width for column alignment (wraps in `knob_cell_for`).
    #[serde(default)]
    pub cell_width: Option<f32>,
    /// Grey this control out when the condition is false. See [`EnableCond`]:
    /// the cell keeps its space, so neighbours never shift.
    #[serde(default)]
    pub enabled_when: Option<EnableCond>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlKind {
    /// A rotary knob over an f32 field. `Log`/`Bipolar` use `min`/`max`.
    Knob {
        #[serde(default)]
        curve: Curve,
        #[serde(default)]
        min: f32,
        #[serde(default)]
        max: f32,
        #[serde(default)]
        unit: Unit,
        /// Optional named formatter id for a `Custom`-supplied display closure.
        #[serde(default)]
        fmt: Option<String>,
        /// Readout NAMES for a knob that sweeps a continuous field but reads
        /// out as one of a short list (a VOWEL knob is 0..1 in the
        /// DSP and shows "AH"/"EH"/"IY"/"OH"/"OO").
        ///
        /// A `Selector` cannot replace it: the field really is continuous, the
        /// mod matrix sweeps it, and quantising the VALUE would change the
        /// sound. Only the DISPLAY is quantised, which is why this sits on the
        /// knob rather than becoming a `Unit` variant (Unit is serialised
        /// positionally in binary formats, so it does not take payloads).
        #[serde(default)]
        names: Vec<String>,
    },
    /// A dropdown over an enum-string or int field.
    ///
    /// `options_from` names a list the EDITOR supplies at render time instead of
    /// spelling it out here. Some option lists live in a Rust enum (an effect's
    /// delay types, reverb types, filter modes) and a .ron that copied them
    /// would be a second source of truth that silently goes stale when the enum
    /// gains a variant. Naming the list keeps one.
    Combo {
        #[serde(default)]
        options: Vec<EnumOption>,
        #[serde(default)]
        options_from: Option<String>,
    },
    /// The same options rendered as a `selectable_label` row.
    Selector { options: Vec<EnumOption> },
    /// A numeric entry field over an f32, for values a user types or nudges
    /// rather than sweeps (an oscillator frequency being the case that asked
    /// for it). A knob cannot resolve 0.01 Hz at one end and 10 kHz at the
    /// other, which is why those editors reached for a DragValue by hand.
    Drag {
        #[serde(default)]
        min: f32,
        #[serde(default)]
        max: f32,
        #[serde(default)]
        speed: f32,
        /// Unit suffix shown in the field, e.g. " Hz".
        #[serde(default)]
        suffix: String,
        /// Field width in points.
        #[serde(default)]
        width: Option<f32>,
    },
    /// A checkbox over a bool field.
    Toggle,
    /// A LATCHING BUTTON over a bool field: filled with the section accent when
    /// on, dim when off.
    ///
    /// Not a stylistic twin of `Toggle`. Where a checkbox reads as an option,
    /// a latch reads as a STOP or a footage tab that is pulled in or out, and
    /// several editors group three or four of them in a row (a vocoder's
    /// 16'/8'/4' footings, an organ's drawbar stops). A checkbox row loses both
    /// the grouping and the at-a-glance state, so the vocabulary carries the
    /// button rather than conversions giving it up.
    Latch {
        /// Button width in points (default 34).
        #[serde(default)]
        width: Option<f32>,
        /// Button height in points (default 22).
        #[serde(default)]
        height: Option<f32>,
    },
    /// Four knobs over a nested `EnvParams` object at `param` (attack/decay/
    /// sustain/release).
    Adsr,
    /// Hand-written escape hatch, dispatched to an editor-supplied closure.
    Custom { id: String },
    /// Static separator text (no param).
    Label { text: String },
}

/// One option in a `Combo`/`Selector`: the serialized value (enum variant name
/// or int as a string) plus a pretty display label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumOption {
    pub value: String,
    #[serde(default)]
    pub label: String,
}

impl EnumOption {
    pub fn display(&self) -> &str {
        if self.label.is_empty() { &self.value } else { &self.label }
    }
}
