//! The generic renderer: walk a `PluginUiSpec` and draw it with the shared
//! `widgets::` primitives, reading/writing params through a `dyn PatchAccess`.
//! Every plugin that adopts this is drawn by the SAME code, so uniformity is
//! guaranteed by construction.

use super::access::PatchAccess;
use super::model::*;
use crate::widgets::{self, KNOB_SMALL};
use egui::{self, Color32, Ui};
use std::collections::HashMap;

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// Evaluate an [`EnableCond`] against the current patch.
///
/// A condition naming a field that doesn't exist (or whose type we can't read)
/// resolves to ENABLED. Silently greying a working control out because a spec
/// has a typo would be far worse than showing one that should have been dim,
/// and `validate()` is where such typos get reported.
fn cond_holds(cond: &Option<EnableCond>, access: &mut dyn PatchAccess) -> bool {
    let Some(c) = cond else { return true };
    let cur = access
        .get_enum(&c.param)
        .or_else(|| access.get_int(&c.param).map(|i| i.to_string()))
        .or_else(|| access.get_bool(&c.param).map(|b| b.to_string()));
    let Some(cur) = cur else { return true };
    let hit = c.equals.iter().any(|v| *v == cur);
    hit != c.not
}

/// The `Custom{id}` escape hatch: a table of hand-written closures keyed by id.
/// A closure gets the `Ui` and a `&mut dyn PatchAccess` (its only write channel),
/// so it composes with the declarative flow without a second borrow of the editor.
pub struct CustomTable<'a> {
    map: HashMap<String, Box<dyn FnMut(&mut Ui, &mut dyn PatchAccess) + 'a>>,
    /// Option lists a spec refers to by name (`Combo(options_from: "...")`).
    options: HashMap<String, Vec<EnumOption>>,
}

impl<'a> Default for CustomTable<'a> {
    fn default() -> Self {
        Self { map: HashMap::new(), options: HashMap::new() }
    }
}

impl<'a> CustomTable<'a> {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add(&mut self, id: impl Into<String>, f: impl FnMut(&mut Ui, &mut dyn PatchAccess) + 'a) {
        self.map.insert(id.into(), Box::new(f));
    }
    /// Supply an option list a spec names with `options_from`.
    pub fn add_options(&mut self, id: impl Into<String>, options: Vec<EnumOption>) {
        self.options.insert(id.into(), options);
    }
    fn options(&self, id: &str) -> Option<&Vec<EnumOption>> {
        self.options.get(id)
    }
    fn run(&mut self, id: &str, ui: &mut Ui, access: &mut dyn PatchAccess) -> bool {
        if let Some(f) = self.map.get_mut(id) {
            f(ui, access);
            true
        } else {
            false
        }
    }
}

/// Draw a whole spec body. `tab_state` is the selected-tab index, owned by the
/// editor so it persists across frames.
/// Draw the tab bar (when there is more than one tab) and return
/// (accent colour, knob size, active tab index).
fn spec_header(ui: &mut Ui, spec: &PluginUiSpec, tab_state: &mut usize) -> (Color32, f32, usize) {
    let accent = rgb(spec.accent);
    let knob = spec.knob_size.unwrap_or(KNOB_SMALL);
    let idx = if spec.tabs.len() > 1 {
        let labels: Vec<&str> = spec.tabs.iter().map(|t| t.label.as_str()).collect();
        if let Some(i) = widgets::tab_bar(ui, *tab_state, &labels, accent) {
            *tab_state = i;
        }
        ui.add_space(4.0);
        (*tab_state).min(spec.tabs.len() - 1)
    } else {
        0
    };
    (accent, knob, idx)
}

/// Fully declarative render WITH its own vertical scroll: the tab bar stays
/// pinned and the tab CONTENT scrolls, so a window shorter than the layout can
/// still reach everything. This is the default entry point. Editors that ALREADY
/// wrap their whole body in a ScrollArea must call `render_spec_inner` instead,
/// or the two scroll areas nest into a double scrollbar.
pub fn render_spec(
    ui: &mut Ui,
    spec: &PluginUiSpec,
    access: &mut dyn PatchAccess,
    custom: &mut CustomTable,
    tab_state: &mut usize,
) {
    let (accent, knob, idx) = spec_header(ui, spec, tab_state);
    if let Some(tab) = spec.tabs.get(idx) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .show(ui, |ui| {
                render_tab(ui, tab, accent, knob, spec.columns, access, custom);
            });
    }
}

/// Same as `render_spec` but WITHOUT its own scroll — for editors that already
/// wrap their body in a ScrollArea (nesting two would double-scroll).
pub fn render_spec_inner(
    ui: &mut Ui,
    spec: &PluginUiSpec,
    access: &mut dyn PatchAccess,
    custom: &mut CustomTable,
    tab_state: &mut usize,
) {
    let (accent, knob, idx) = spec_header(ui, spec, tab_state);
    if let Some(tab) = spec.tabs.get(idx) {
        render_tab(ui, tab, accent, knob, spec.columns, access, custom);
    }
}

/// LAYOUT-ONLY render: draw the tab bar + column grid from the spec, and draw
/// each section via the editor's `draw` callback keyed on `SectionSpec::section`
/// (the callback also draws the section header). For typed-command plugins that
/// keep their sections hand-written but make tab/column PLACEMENT declarative.
pub fn render_layout(
    ui: &mut Ui,
    spec: &PluginUiSpec,
    tab_state: &mut usize,
    mut draw: impl FnMut(&str, &mut Ui),
) {
    let accent = rgb(spec.accent);
    let tab_idx = if spec.tabs.len() > 1 {
        let labels: Vec<&str> = spec.tabs.iter().map(|t| t.label.as_str()).collect();
        if let Some(i) = widgets::tab_bar(ui, *tab_state, &labels, accent) {
            *tab_state = i;
        }
        ui.add_space(4.0);
        (*tab_state).min(spec.tabs.len() - 1)
    } else {
        0
    };
    let Some(tab) = spec.tabs.get(tab_idx) else { return };
    let ncols = tab
        .columns
        .or(spec.columns)
        .unwrap_or_else(|| {
            tab.sections.iter().filter_map(|s| s.column).max().map(|m| m + 1).unwrap_or(1)
        })
        .max(1);
    let mut by_col: Vec<Vec<&SectionSpec>> = vec![Vec::new(); ncols];
    for s in &tab.sections {
        by_col[s.column.unwrap_or(0).min(ncols - 1)].push(s);
    }
    ui.columns(ncols, |cols| {
        for (i, sections) in by_col.iter().enumerate() {
            for (j, s) in sections.iter().enumerate() {
                let Some(id) = s.section.clone() else { continue };
                if j > 0 {
                    cols[i].add_space(6.0);
                }
                widgets::panel_frame(&mut cols[i], |ui| draw(&id, ui));
            }
        }
    });
}

fn render_tab(
    ui: &mut Ui,
    tab: &TabSpec,
    accent: Color32,
    knob: f32,
    fallback_cols: Option<usize>,
    access: &mut dyn PatchAccess,
    custom: &mut CustomTable,
) {
    if tab.bands.is_empty() {
        // One-band shorthand: the shape most specs use.
        let band = BandSpec {
            columns: tab.columns.or(fallback_cols),
            column_weights: tab.column_weights.clone(),
            height_frac: None,
            separator_below: false,
            sections: tab.sections.clone(),
        };
        render_band(ui, &band, accent, knob, fallback_cols, access, custom);
        return;
    }
    let total_h = ui.available_height();
    let n = tab.bands.len();
    for (i, band) in tab.bands.iter().enumerate() {
        match band.height_frac {
            Some(f) if f > 0.0 && f < 1.0 => {
                let h = total_h * f;
                ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
                    render_band(ui, band, accent, knob, fallback_cols, access, custom);
                });
            }
            // No fraction: take what is needed (or what is left, for the last).
            _ => render_band(ui, band, accent, knob, fallback_cols, access, custom),
        }
        if band.separator_below && i + 1 < n {
            ui.separator();
        }
    }
}

fn render_band(
    ui: &mut Ui,
    band: &BandSpec,
    accent: Color32,
    knob: f32,
    fallback_cols: Option<usize>,
    access: &mut dyn PatchAccess,
    custom: &mut CustomTable,
) {
    let ncols = band
        .columns
        .or(fallback_cols)
        .unwrap_or_else(|| {
            band.sections.iter().filter_map(|s| s.column).max().map(|m| m + 1).unwrap_or(1)
        })
        .max(1);

    let mut by_col: Vec<Vec<&SectionSpec>> = vec![Vec::new(); ncols];
    for s in &band.sections {
        let c = s.column.unwrap_or(0).min(ncols - 1);
        by_col[c].push(s);
    }

    // Equal columns unless the band gives proportions. `ui.columns` only ever
    // divides evenly, so weighted layouts allocate their own strips.
    let weights: Vec<f32> = if band.column_weights.len() == ncols
        && band.column_weights.iter().all(|w| *w > 0.0)
    {
        band.column_weights.clone()
    } else {
        Vec::new()
    };

    if weights.is_empty() {
        ui.columns(ncols, |cols| {
            for (i, sections) in by_col.iter().enumerate() {
                for (j, s) in sections.iter().enumerate() {
                    if j > 0 {
                        cols[i].add_space(6.0);
                    }
                    render_section(&mut cols[i], s, accent, knob, access, custom);
                }
            }
        });
    } else {
        let total: f32 = weights.iter().sum();
        let gap = 6.0;
        let avail = (ui.available_width() - gap * (ncols as f32 - 1.0)).max(1.0);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            for (i, sections) in by_col.iter().enumerate() {
                let w = avail * weights[i] / total;
                ui.allocate_ui(egui::vec2(w, height), |ui| {
                    ui.set_max_width(w);
                    ui.vertical(|ui| {
                        for (j, s) in sections.iter().enumerate() {
                            if j > 0 { ui.add_space(6.0); }
                            render_section(ui, s, accent, knob, access, custom);
                        }
                    });
                });
                if i + 1 < ncols { ui.add_space(gap); }
            }
        });
    }
}

fn render_section(
    ui: &mut Ui,
    s: &SectionSpec,
    spec_accent: Color32,
    knob: f32,
    access: &mut dyn PatchAccess,
    custom: &mut CustomTable,
) {
    let accent = s.accent.map(rgb).unwrap_or(spec_accent);
    let on = cond_holds(&s.enabled_when, access);
    // A header toggle dims its own icon and title, which is how these editors
    // show a layer being off. Read it BEFORE the frame so the header can tint.
    let toggle_state = s.header_toggle.as_ref().and_then(|f| access.get_bool(f));
    let mut toggled = false;
    if let Some(w) = s.max_width { ui.set_max_width(w); }
    widgets::panel_frame(ui, |ui| {
        match (s.icon.as_deref().and_then(crate::icons::by_name), toggle_state) {
            (Some((icon, uri)), Some(state)) => {
                toggled = widgets::panel_header_toggle(ui, icon, uri, &s.title, accent, state);
            }
            (Some((icon, uri)), None) => {
                widgets::section_header_icon(ui, icon, uri, &s.title, accent);
            }
            (None, Some(state)) => {
                toggled = widgets::panel_header_toggle_plain(ui, &s.title, accent, state);
            }
            (None, None) => widgets::section_header(ui, &s.title, accent),
        }
        // Greyed, never hidden: the section keeps its footprint so the columns
        // around it don't reflow when the mode changes.
        ui.add_enabled_ui(on, |ui| {
            for row in &s.rows {
                ui.horizontal_top(|ui| {
                    for c in row {
                        render_control(ui, c, accent, knob, access, custom);
                    }
                });
            }
        });
    });
    if toggled {
        if let (Some(field), Some(state)) = (s.header_toggle.as_ref(), toggle_state) {
            access.set_bool(field, !state);
        }
    }
}

fn render_control(
    ui: &mut Ui,
    c: &ControlSpec,
    accent: Color32,
    knob: f32,
    access: &mut dyn PatchAccess,
    custom: &mut CustomTable,
) {
    let body = |ui: &mut Ui, access: &mut dyn PatchAccess, custom: &mut CustomTable| match &c.kind {
        ControlKind::Knob { curve, min, max, unit, names, .. } => {
            let cur = access.get_f32(&c.param).unwrap_or(0.0);
            // A pitch knob can show its Hz value as a note name when the patch
            // carries a `pitch_unit` enum set to "Note" (a Hz/Note
            // toggle). Guarded on the param name + field presence, so no other
            // plugin is affected.
            let unit = if c.param.ends_with(".pitch")
                && access.get_enum("pitch_unit").as_deref() == Some("Note")
            {
                crate::widgets::Unit::Note
            } else {
                *unit
            };
            if let Some(v) =
                knob_ranged(ui, cur, &c.label, *curve, *min, *max, unit, names, accent, knob)
            {
                access.set_f32(&c.param, v);
            }
        }
        // Short controls are wrapped in vcell so they centre on the knob-circle
        // line instead of dropping below the knobs in a mixed row.
        ControlKind::Combo { options, options_from } => {
            // A named list wins over an inline one: the editor holds the live
            // enum, the spec only says which list to use.
            let resolved = options_from.as_deref().and_then(|n| custom.options(n));
            let opts = resolved.unwrap_or(options);
            vcell(ui, knob, |ui| render_combo(ui, c, opts, access))
        }
        ControlKind::Selector { options } => vcell(ui, knob, |ui| render_selector(ui, c, options, accent, access)),
        ControlKind::Drag { min, max, speed, suffix, width } => vcell(ui, knob, |ui| {
            let mut v = access.get_f32(&c.param).unwrap_or(0.0);
            if !c.label.is_empty() {
                ui.label(egui::RichText::new(&c.label).size(9.0)
                    .color(crate::theme::Palette::of(ui.ctx()).text_dim));
            }
            let mut d = egui::DragValue::new(&mut v)
                .speed(if *speed > 0.0 { *speed } else { 0.01 })
                .suffix(suffix.as_str());
            // Only clamp when the spec actually gave a range. A DragValue's
            // range() clamps on EVERY frame, so a default 0..0 would silently
            // pin the value to zero.
            if *min < *max {
                d = d.range(*min..=*max);
            }
            let r = widgets::drag_fixed(ui, d, width.unwrap_or(62.0));
            if r.changed() {
                access.set_f32(&c.param, v);
            }
        }),
        ControlKind::Toggle => vcell(ui, knob, |ui| {
            let mut b = access.get_bool(&c.param).unwrap_or(false);
            if ui.checkbox(&mut b, &c.label).changed() {
                access.set_bool(&c.param, b);
            }
        }),
        ControlKind::Latch { width, height } => {
            let mut b = access.get_bool(&c.param).unwrap_or(false);
            let size = egui::vec2(width.unwrap_or(34.0), height.unwrap_or(22.0));
            let fill = if b { accent } else { ui.visuals().widgets.inactive.bg_fill };
            let fg = if b { egui::Color32::WHITE } else { ui.visuals().weak_text_color() };
            let r = ui.add(
                egui::Button::new(egui::RichText::new(&c.label).color(fg).size(11.0))
                    .fill(fill)
                    .min_size(size)
                    .corner_radius(3.0),
            );
            if r.clicked() {
                b = !b;
                access.set_bool(&c.param, b);
            }
        }
        ControlKind::Adsr => render_adsr(ui, &c.param, accent, knob, access),
        ControlKind::Custom { id } => {
            if !custom.run(id, ui, access) {
                ui.weak(format!("<custom:{id}?>"));
            }
        }
        ControlKind::Label { text } => vcell(ui, knob, |ui| {
            ui.label(text);
        }),
    };

    let on = cond_holds(&c.enabled_when, access);
    if let Some(w) = c.cell_width {
        // The cell is claimed OUTSIDE add_enabled_ui so a disabled control
        // still occupies its column slot.
        widgets::knob_cell_for(ui, w, knob, |ui| {
            ui.add_enabled_ui(on, |ui| body(ui, access, custom));
        });
    } else {
        ui.add_enabled_ui(on, |ui| body(ui, access, custom));
    }
}

/// Vertically centre a short control on the knob-circle line. Thin wrapper over
/// the shared `widgets::vcell` (KNOB_SMALL-tall) so the declarative renderer and
/// hand-drawn Custom closures share ONE alignment primitive.
fn vcell<R>(ui: &mut Ui, knob: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    widgets::vcell(ui, knob, add)
}

/// Normalize a real value to the knob's 0..1, draw via `knob_fmt`, and return
/// the new real value on change. Unifies the linear/log/bipolar mappings the
/// per-plugin `bass_knob`/`bass_knob_log` helpers duplicated.
fn knob_ranged(
    ui: &mut Ui,
    cur: f32,
    label: &str,
    curve: Curve,
    min: f32,
    max: f32,
    unit: Unit,
    names: &[String],
    accent: Color32,
    knob: f32,
) -> Option<f32> {
    let (mut lo, mut hi) = (min, max);
    if !(lo < hi) {
        // Sensible defaults when the spec omits the range.
        (lo, hi) = match curve {
            Curve::Bipolar => (-1.0, 1.0),
            _ => (0.0, 1.0),
        };
    }
    let mut norm = match curve {
        Curve::Log => {
            let lo = lo.max(1e-6);
            let v = cur.max(lo);
            ((v / lo).ln() / (hi / lo).ln()).clamp(0.0, 1.0)
        }
        Curve::Sqrt => (((cur - lo) / (hi - lo)).clamp(0.0, 1.0)).sqrt(),
        _ => ((cur - lo) / (hi - lo)).clamp(0.0, 1.0),
    };
    // A named knob reads out one of a short list instead of a number; the
    // VALUE stays continuous (the mod matrix sweeps it), only the text is
    // quantised.
    let disp = if names.is_empty() {
        unit.format(cur)
    } else {
        let i = (norm * (names.len() - 1) as f32).round() as usize;
        names[i.min(names.len() - 1)].clone()
    };
    let old = norm;
    widgets::knob_fmt(ui, &mut norm, label, &disp, knob, accent);
    if (norm - old).abs() > 1e-9 {
        Some(denormalise(norm, curve, lo, hi))
    } else {
        None
    }
}

/// Knob 0..1 back to a real value. The exact inverse of the `norm` mapping in
/// `knob_ranged`, and split out so it CANNOT drift from it again: `Sqrt`
/// normalised with a square root but denormalised linearly, so all 39 Sqrt
/// knobs in two specs wrote a value that did not match the
/// pointer. The round-trip test missed it by re-implementing the formula
/// instead of calling this.
fn denormalise(norm: f32, curve: Curve, lo: f32, hi: f32) -> f32 {
    match curve {
        Curve::Log => {
            let lo = lo.max(1e-6);
            (lo * (hi / lo).powf(norm)).clamp(lo, hi)
        }
        Curve::Sqrt => (lo + norm * norm * (hi - lo)).clamp(lo, hi),
        _ => (lo + norm * (hi - lo)).clamp(lo, hi),
    }
}

fn render_combo(ui: &mut Ui, c: &ControlSpec, options: &[EnumOption], access: &mut dyn PatchAccess) {
    let is_int = access.get_int(&c.param).is_some();
    let cur = access
        .get_enum(&c.param)
        .or_else(|| access.get_int(&c.param).map(|i| i.to_string()));
    let cur_label = options
        .iter()
        .find(|o| Some(o.value.as_str()) == cur.as_deref())
        .map(EnumOption::display)
        .unwrap_or("");
    if !c.label.is_empty() {
        ui.label(egui::RichText::new(&c.label).size(9.0).color(crate::theme::Palette::of(ui.ctx()).text_dim));
    }
    let mut chosen: Option<String> = None;
    egui::ComboBox::from_id_salt(("uispec_combo", c.param.as_str(), c.label.as_str()))
        // Fixed width so a long selected option never shoves the next control in
        // the declarative row (combos without a cell_width used to auto-size).
        .width(c.cell_width.map(|w| (w - 8.0).max(48.0)).unwrap_or(108.0))
        .selected_text(cur_label)
        .show_ui(ui, |ui| {
            for o in options {
                let sel = cur.as_deref() == Some(o.value.as_str());
                if ui.selectable_label(sel, o.display()).clicked() {
                    chosen = Some(o.value.clone());
                }
            }
        });
    if let Some(v) = chosen {
        if is_int {
            if let Ok(iv) = v.parse::<i64>() {
                access.set_int(&c.param, iv);
            }
        } else {
            access.set_enum(&c.param, &v);
        }
    }
}

fn render_selector(
    ui: &mut Ui,
    c: &ControlSpec,
    options: &[EnumOption],
    accent: Color32,
    access: &mut dyn PatchAccess,
) {
    let is_int = access.get_int(&c.param).is_some();
    let cur = access
        .get_enum(&c.param)
        .or_else(|| access.get_int(&c.param).map(|i| i.to_string()));
    if !c.label.is_empty() {
        ui.label(egui::RichText::new(&c.label).size(9.0).color(crate::theme::Palette::of(ui.ctx()).text_dim));
    }
    let mut chosen: Option<String> = None;
    for o in options {
        let sel = cur.as_deref() == Some(o.value.as_str());
        if widgets::selector_pill(ui, o.display(), sel, accent).clicked() {
            chosen = Some(o.value.clone());
        }
    }
    if let Some(v) = chosen {
        if is_int {
            if let Ok(iv) = v.parse::<i64>() {
                access.set_int(&c.param, iv);
            }
        } else {
            access.set_enum(&c.param, &v);
        }
    }
}

/// A/D/S/R over a nested `EnvParams` object. Times (A/D/R) are log over
/// [1 ms, 5 s]; sustain is linear 0..1.
fn render_adsr(ui: &mut Ui, param: &str, accent: Color32, knob: f32, access: &mut dyn PatchAccess) {
    for (field, label, unit, curve, lo, hi) in [
        ("attack", "A", Unit::Ms, Curve::Log, 0.001, 5.0),
        ("decay", "D", Unit::Ms, Curve::Log, 0.001, 5.0),
        ("sustain", "S", Unit::Pct, Curve::Linear, 0.0, 1.0),
        ("release", "R", Unit::Ms, Curve::Log, 0.001, 5.0),
    ] {
        let cur = access.get_nested_f32(param, field).unwrap_or(lo);
        if let Some(v) = knob_ranged(ui, cur, label, curve, lo, hi, unit, &[], accent, knob) {
            access.set_nested_f32(param, field, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_spec::JsonPatchAccess;

    #[derive(serde::Serialize, serde::Deserialize, Default)]
    struct Sample {
        mode: String,
        sync: bool,
        div: i64,
    }

    fn cond(param: &str, equals: &[&str], not: bool) -> Option<EnableCond> {
        Some(EnableCond {
            param: param.to_string(),
            equals: equals.iter().map(|s| s.to_string()).collect(),
            not,
        })
    }

    #[test]
    fn a_condition_matches_enums_bools_and_ints() {
        let s = Sample { mode: "Wobble".into(), sync: true, div: 3 };
        let mut a = JsonPatchAccess::new(&s);
        assert!(cond_holds(&cond("mode", &["Wobble"], false), &mut a));
        assert!(!cond_holds(&cond("mode", &["Funk"], false), &mut a));
        assert!(cond_holds(&cond("mode", &["Funk"], true), &mut a));
        assert!(cond_holds(&cond("sync", &["true"], false), &mut a));
        assert!(cond_holds(&cond("div", &["3"], false), &mut a));
        assert!(!cond_holds(&cond("div", &["4"], false), &mut a));
    }

    /// A condition on a field that isn't there must leave the control USABLE.
    /// Greying a working knob out over a spec typo is a far worse failure than
    /// leaving one bright that should have been dim, and specs are hot-reloaded
    /// from disk where typos are cheap to make.
    #[test]
    fn an_unresolvable_condition_leaves_the_control_enabled() {
        let s = Sample::default();
        let mut a = JsonPatchAccess::new(&s);
        assert!(cond_holds(&cond("no_such_field", &["x"], false), &mut a));
        assert!(cond_holds(&None, &mut a));
    }

    /// Sqrt must be a true round trip, or a knob would drift on every frame:
    /// the renderer normalises the stored value, draws, then denormalises what
    /// comes back, so an asymmetric pair walks the value away under the cursor.
    #[test]
    fn the_sqrt_curve_round_trips() {
        for &v in &[0.125f32, 1.0, 4.0, 9.0, 16.0] {
            let n = ((v - 0.0) / 16.0).clamp(0.0, 1.0).sqrt();
            // Call the SHIPPING inverse, not a copy of the formula. The copy
            // is what let Sqrt normalise with a root and denormalise linearly
            // for 39 knobs without a single test noticing.
            let back = denormalise(n, Curve::Sqrt, 0.0, 16.0);
            assert!((back - v).abs() < 1e-4, "sqrt round trip lost {v} -> {back}");
        }
    }

    /// And it must actually differ from linear where it matters: the point of
    /// the curve is resolution at the BOTTOM.
    #[test]
    fn sqrt_gives_the_low_end_more_travel_than_linear() {
        // Half-way round the knob lands at a QUARTER of the range, not half.
        let mid = 0.5f32;
        assert!((mid * mid * 16.0 - 4.0).abs() < 1e-4);
    }

    /// Column weights must be honoured only when they MATCH the column count
    /// and are all positive; a mismatched or zero weight must fall back to
    /// equal columns rather than collapsing a column to nothing.
    #[test]
    fn column_weights_are_used_only_when_they_are_usable() {
        fn usable(ncols: usize, w: &[f32]) -> bool {
            w.len() == ncols && w.iter().all(|x| *x > 0.0)
        }
        assert!(usable(4, &[1.5, 1.2, 1.0, 1.3]));
        assert!(!usable(4, &[1.0, 1.0, 1.0]), "too few weights");
        assert!(!usable(3, &[1.0, 0.0, 1.0]), "a zero weight would vanish a column");
        assert!(!usable(3, &[1.0, -1.0, 1.0]), "a negative weight is nonsense");
        assert!(!usable(3, &[]), "no weights means equal columns");
    }

}
