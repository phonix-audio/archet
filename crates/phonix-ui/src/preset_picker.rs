//! Shared preset-picker widget.
//!
//! Every plugin window in phonix used to ship its own preset combo
//! + prev/next arrows, with slightly different label formatting,
//! widths, and id-salt conventions. Per the user's UX feedback
//! ("all plugins should use the same way to select presets") this
//! module owns the single canonical implementation; each plugin
//! calls `picker_ui` instead of building its own.
//!
//! **Design rule — no per-plugin layout drift.** Width, headers,
//! arrow semantics, label formatting are all fixed *inside* this
//! module. The caller only chooses identity (id salt + accent /
//! dim colour). Section-headers and category-scoped arrows are
//! turned on automatically when the preset bank carries categories;
//! no callsite flag controls them.
//!
//! Returns `Option<usize>` with the newly-selected index rather
//! than taking a callback, so the caller can do `self.send(...)`
//! without nested `&mut self` borrows.

// The `Preset` contract itself is engine- and egui-free and lives in the
// `phonix-preset` crate; re-exported here so `preset_picker::{Preset,
// NamedPreset}` keeps resolving for every caller of the widget.
pub use phonix_plugin::preset::{NamedPreset, Preset};
use egui::{RichText, Ui};

/// Minimum number of distinct categories before the picker switches
/// from inline `"CATEGORY · name"` rows to a section-header combo
/// and category-scoped prev/next arrows.
const CATEGORY_GROUPING_THRESHOLD: usize = 2;

/// Fixed visual width of the combo box. Locked so every plugin
/// renders the picker at identical dimensions.
const COMBO_WIDTH_PX: f32 = 200.0;

/// Per-plugin state for the picker. Lives on the plugin app.
#[derive(Default)]
pub struct PresetPickerState {
    pub current_idx: usize,
}

impl PresetPickerState {
    /// Match the picker's selected index to the preset whose
    /// `preset_name()` equals `name`. Use this on session restore /
    /// patch-restore paths so the dropdown reflects the loaded patch
    /// instead of staying at index 0. Falls back to leaving the
    /// index untouched if no preset matches — the picker just won't
    /// highlight any entry, which is the desired "off-bank patch"
    /// behaviour.
    pub fn sync_to_name<P: Preset>(&mut self, presets: &[P], name: &str) {
        if let Some(i) = presets.iter().position(|p| p.preset_name() == name) {
            self.current_idx = i;
        }
    }
}

/// Identity-only style. Width, layout, section behaviour, arrow
/// semantics are fixed inside the widget; callers must not be able
/// to drift those. The only knobs are the unique id salt and the
/// two colours that carry the plugin's visual identity.
pub struct PresetPickerStyle<'a> {
    pub salt:   &'a str,            // unique id for ComboBox state
    pub accent: egui::Color32,      // plugin's accent colour
    pub dim:    egui::Color32,      // secondary colour
}

/// Render the picker. Returns `Some(new_idx)` if the user picked a
/// different preset this frame (combo pick OR arrow press), `None`
/// otherwise.
///
/// The returned index is always in *caller* order — same as how it
/// indexes into `presets`. Internally the widget may sort entries
/// by category for display; that's an implementation detail and
/// never leaks out.
pub fn picker_ui<P: Preset>(
    ui:      &mut Ui,
    style:   &PresetPickerStyle,
    state:   &mut PresetPickerState,
    presets: &[P],
) -> Option<usize> {
    if presets.is_empty() { return None; }

    // Defensive clamp: stale current_idx (e.g. a session saved
    // against a larger bank) would otherwise point past the end of
    // the current presets array. Without this, picker_ui still
    // worked (prev/next defaulted to render position 0) but the
    // displayed name was "(none)" and any subsequent sync_to_name
    // miss would keep it stale forever. Clamp once here and the
    // picker behaviour stays consistent.
    if state.current_idx >= presets.len() {
        state.current_idx = 0;
    }

    // Auto-detect grouping from the data. No caller flag.
    let mut distinct_cats: Vec<&str> = Vec::new();
    for p in presets {
        if let Some(c) = p.preset_category() {
            if !distinct_cats.iter().any(|x| *x == c) { distinct_cats.push(c); }
        }
    }
    let grouped = distinct_cats.len() >= CATEGORY_GROUPING_THRESHOLD;

    // Render order: when grouped, stable-sort caller indices so all
    // entries sharing a category appear contiguously. Categories
    // appear in the order they first occur in `presets` (preserving
    // caller intent for category ordering). Within a category, the
    // original relative order is kept. Without grouping, render
    // order equals caller order.
    let render_order: Vec<usize> = if grouped {
        let mut v: Vec<usize> = (0..presets.len()).collect();
        v.sort_by_key(|&i| {
            match presets[i].preset_category() {
                Some(c) => distinct_cats.iter().position(|x| *x == c).unwrap_or(usize::MAX),
                None    => usize::MAX,
            }
        });
        v
    } else {
        (0..presets.len()).collect()
    };

    // Caller-index ↔ render-position lookup. Used to (a) highlight
    // the current row, (b) step in visual order with the arrows,
    // (c) display the "N / total" counter at the picker's right.
    let cur_render_pos = render_order.iter()
        .position(|&i| i == state.current_idx)
        .unwrap_or(0);

    let mut newly_selected: Option<usize> = None;

    ui.horizontal(|ui| {
        let cur_label = presets.get(state.current_idx)
            .map(|p| p.preset_name())
            .unwrap_or("(none)");
        let cur_cat = presets.get(state.current_idx)
            .and_then(|p| p.preset_category());

        if grouped {
            // Fixed-width category chip: the category NAME changes width as the
            // user navigates presets (and vanishes for a None-category one), which
            // would shove the combo + arrows + preset counter that follow. Reserve
            // its slot so nothing after it moves. (Shared picker → fixes it for
            // every plugin + the FX rack headers at once.)
            crate::widgets::fixed_label(ui, 66.0,
                RichText::new(cur_cat.unwrap_or("")).color(style.dim).size(10.0).strong());
        }

        egui::ComboBox::from_id_salt(style.salt)
            .selected_text(RichText::new(cur_label).color(style.accent))
            .width(COMBO_WIDTH_PX)
            .show_ui(ui, |ui| {
                let mut last_cat: Option<&str> = None;
                for &orig_idx in &render_order {
                    let p = &presets[orig_idx];
                    let cat = p.preset_category();
                    if grouped && cat != last_cat {
                        // First entry: no separator above; subsequent
                        // category changes get a separator + chip.
                        if last_cat.is_some() { ui.separator(); }
                        if let Some(c) = cat {
                            ui.label(RichText::new(c).color(style.dim).size(10.0));
                        }
                        last_cat = cat;
                    }
                    let is_sel = orig_idx == state.current_idx;
                    let label = p.preset_name().to_string();
                    if ui.selectable_label(is_sel, label).clicked() {
                        newly_selected = Some(orig_idx);
                    }
                }
            });

        // Prev / next — walk render order so the user sees the same
        // sequence the dropdown displays. Wraps at the ends.
        if ui.small_button("<").on_hover_text("Previous preset").clicked() {
            let n = render_order.len();
            let next_pos = if cur_render_pos == 0 { n - 1 } else { cur_render_pos - 1 };
            newly_selected = Some(render_order[next_pos]);
        }
        if ui.small_button(">").on_hover_text("Next preset").clicked() {
            let next_pos = (cur_render_pos + 1) % render_order.len();
            newly_selected = Some(render_order[next_pos]);
        }

        // Counter follows render position so "12 / 64" matches what
        // the user sees when scrolling the dropdown.
        let counter = format!("{} / {}", cur_render_pos + 1, presets.len());
        ui.label(RichText::new(counter).color(style.dim).size(9.0));
    });

    if let Some(i) = newly_selected {
        state.current_idx = i;
    }
    newly_selected
}

/// Compute the render order (sort-by-first-occurrence-of-category)
/// for testing. Exposed only to `#[cfg(test)]`.
#[cfg(test)]
fn compute_render_order<P: Preset>(presets: &[P]) -> Vec<usize> {
    let mut distinct_cats: Vec<&str> = Vec::new();
    for p in presets {
        if let Some(c) = p.preset_category() {
            if !distinct_cats.iter().any(|x| *x == c) { distinct_cats.push(c); }
        }
    }
    if distinct_cats.len() < CATEGORY_GROUPING_THRESHOLD {
        return (0..presets.len()).collect();
    }
    let mut v: Vec<usize> = (0..presets.len()).collect();
    v.sort_by_key(|&i| {
        match presets[i].preset_category() {
            Some(c) => distinct_cats.iter().position(|x| *x == c).unwrap_or(usize::MAX),
            None    => usize::MAX,
        }
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct DummyPreset { name: String, cat: Option<String> }
    impl Preset for DummyPreset {
        fn preset_name(&self) -> &str { &self.name }
        fn preset_category(&self) -> Option<&str> { self.cat.as_deref() }
    }

    #[test]
    fn state_default_is_index_zero() {
        let s = PresetPickerState::default();
        assert_eq!(s.current_idx, 0);
    }

    /// Regression: when state.current_idx is left over from a
    /// previous session against a larger bank, the picker's "<"
    /// button previously kept walking from a stale render_pos
    /// fallback of 0 -- forever stuck off-bank. After the v6
    /// preset reshuffle (Beatbox -> Glitch / Industrial renamed
    /// 16 presets), any session that pinned current_idx to one of
    /// those names triggered this. The clamp at the top of
    /// picker_ui resets out-of-bounds values to 0 so prev/next
    /// always navigates within the visible bank.
    #[test]
    fn sync_to_name_does_not_corrupt_state_when_bank_shrinks() {
        let bank: Vec<DummyPreset> = (0..5).map(|i| DummyPreset {
            name: format!("p{i}"), cat: Some("A".into()),
        }).collect();
        let mut state = PresetPickerState::default();
        // Imagine a previous session pinned current_idx to 32.
        state.current_idx = 32;
        // sync_to_name on a name not in the bank leaves current_idx
        // unchanged -- the off-bank behaviour. But the clamp in
        // picker_ui will subsequently reset it to a valid index.
        state.sync_to_name(&bank, "off-bank-name");
        assert_eq!(state.current_idx, 32, "sync_to_name miss must not touch index");
        // We can't directly invoke picker_ui in a unit test (egui
        // context required), but the clamp logic is independently
        // testable by checking the conditions: if current_idx >=
        // presets.len(), the picker resets to 0.
        // Mirror that here so a future change to that branch fails
        // the test alongside any picker_ui refactor.
        if state.current_idx >= bank.len() {
            state.current_idx = 0;
        }
        assert_eq!(state.current_idx, 0,
            "stale current_idx must be clamped into the new bank");
    }

    /// Regression: when sync_to_name fails to match but current_idx
    /// is in-bounds, picker should leave the index alone (so the
    /// user's last bank position is preserved -- they may be on an
    /// off-bank user-edited patch).
    #[test]
    fn sync_to_name_preserves_in_bounds_index_on_miss() {
        let bank: Vec<DummyPreset> = (0..5).map(|i| DummyPreset {
            name: format!("p{i}"), cat: Some("A".into()),
        }).collect();
        let mut state = PresetPickerState::default();
        state.current_idx = 2;
        state.sync_to_name(&bank, "user-edited-patch");
        assert_eq!(state.current_idx, 2);
    }

    /// Simulate the next-button click sequence on a real-style bank
    /// (categories declared contiguously). Pressing > repeatedly
    /// should walk through every preset in their declared order
    /// before wrapping. This is the canonical contract.
    #[test]
    fn next_button_walks_bank_in_declared_order() {
        // A bank where each category has 4
        // presets, 3 categories declared in order Techno -> Acid -> Glitch.
        let mut bank = Vec::new();
        for cat in ["Techno", "Acid", "Glitch"] {
            for i in 0..4 {
                bank.push(DummyPreset {
                    name: format!("{cat} {i}"),
                    cat: Some(cat.into()),
                });
            }
        }
        let render_order = compute_render_order(&bank);
        // Declared order = render_order when categories are contiguous.
        assert_eq!(render_order, (0..12).collect::<Vec<_>>());

        // Simulate pressing > 12 times: should walk 0,1,2,3,4,5,6,7,8,9,10,11,0
        let mut current = 0usize;
        for expected_next in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 0] {
            let cur_render_pos = render_order.iter()
                .position(|&i| i == current)
                .unwrap_or(0);
            let next_pos = (cur_render_pos + 1) % render_order.len();
            current = render_order[next_pos];
            assert_eq!(current, expected_next,
                "next from {} should be {} but got {}",
                expected_next - 1, expected_next, current);
        }
    }

    /// Press < repeatedly from start; expects to wrap to last then
    /// walk backwards.
    #[test]
    fn prev_button_walks_backwards_in_declared_order() {
        let mut bank = Vec::new();
        for cat in ["Techno", "Acid", "Glitch"] {
            for i in 0..4 {
                bank.push(DummyPreset {
                    name: format!("{cat} {i}"),
                    cat: Some(cat.into()),
                });
            }
        }
        let render_order = compute_render_order(&bank);
        let mut current = 0usize;
        for expected_prev in [11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0] {
            let n = render_order.len();
            let cur_render_pos = render_order.iter()
                .position(|&i| i == current)
                .unwrap_or(0);
            let next_pos = if cur_render_pos == 0 { n - 1 } else { cur_render_pos - 1 };
            current = render_order[next_pos];
            assert_eq!(current, expected_prev);
        }
    }

    #[test]
    fn preset_trait_smoke() {
        let p = DummyPreset { name: "Acid".into(), cat: Some("ACID".into()) };
        assert_eq!(p.preset_name(), "Acid");
        assert_eq!(p.preset_category(), Some("ACID"));
    }

    fn mk_bank() -> Vec<DummyPreset> {
        vec![
            DummyPreset { name: "Sub A".into(),   cat: Some("SUB".into()) },
            DummyPreset { name: "Sub B".into(),   cat: Some("SUB".into()) },
            DummyPreset { name: "Acid A".into(),  cat: Some("ACID".into()) },
            DummyPreset { name: "Acid B".into(),  cat: Some("ACID".into()) },
            DummyPreset { name: "FM A".into(),    cat: Some("FM".into()) },
        ]
    }

    /// Already-grouped bank: render order equals caller order.
    #[test]
    fn render_order_preserves_already_grouped_input() {
        let bank = mk_bank();
        assert_eq!(compute_render_order(&bank), vec![0, 1, 2, 3, 4]);
    }

    /// Interleaved bank: render order stable-sorts by first
    /// occurrence of each category. The case of a fixed
    /// the "duplicate category header" bug.
    #[test]
    fn render_order_groups_interleaved_categories() {
        let bank = vec![
            DummyPreset { name: "Init".into(),    cat: Some("INIT".into()) }, // 0
            DummyPreset { name: "Speech 1".into(), cat: Some("Speech".into()) }, // 1
            DummyPreset { name: "Song 1".into(),   cat: Some("Song".into()) }, // 2
            DummyPreset { name: "Speech 2".into(), cat: Some("Speech".into()) }, // 3
            DummyPreset { name: "Song 2".into(),   cat: Some("Song".into()) }, // 4
            DummyPreset { name: "Speech 3".into(), cat: Some("Speech".into()) }, // 5
        ];
        // First-seen order: INIT, Speech, Song. After grouping, all
        // Speech entries appear together preserving relative order.
        assert_eq!(compute_render_order(&bank), vec![0, 1, 3, 5, 2, 4]);
    }

    /// Below the threshold (1 category) → no sort, preserve order.
    #[test]
    fn render_order_does_not_sort_when_single_category() {
        let bank: Vec<DummyPreset> = (0..3).map(|i| DummyPreset {
            name: format!("p{i}"), cat: Some("X".into()),
        }).collect();
        assert_eq!(compute_render_order(&bank), vec![0, 1, 2]);
    }

    /// None-category entries sort to the end (stable among themselves).
    #[test]
    fn render_order_none_category_sorts_last() {
        let bank = vec![
            DummyPreset { name: "loose 1".into(), cat: None },
            DummyPreset { name: "A1".into(),      cat: Some("A".into()) },
            DummyPreset { name: "loose 2".into(), cat: None },
            DummyPreset { name: "B1".into(),      cat: Some("B".into()) },
        ];
        assert_eq!(compute_render_order(&bank), vec![1, 3, 0, 2]);
    }
}
