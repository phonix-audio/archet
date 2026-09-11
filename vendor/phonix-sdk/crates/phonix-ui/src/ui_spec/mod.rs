//! Declarative, hot-reloadable plugin-UI layer.
//!
//! A plugin editor's LAYOUT is described by a `.ron` spec (see [`model`]) loaded
//! at runtime and, in debug builds, hot-reloaded on save. One generic renderer
//! ([`render_spec`]) draws it with the shared `widgets::` primitives, reading and
//! writing params by field name through a [`PatchAccess`] (serde is the reflection
//! Rust lacks). This guarantees UI uniformity by construction and lets layout be
//! iterated WITHOUT recompiling.
//!
//! BOUNDARY: only LAYOUT is data. A spec relocates/relabels/re-curves an EXISTING
//! patch field; a new parameter still needs a Rust field + engine DSP + recompile.
//! The renderer only mutates the GUI-thread patch mirror and sends the plugin's
//! existing `LoadPatch`, so the audio/RT thread is untouched.

pub mod access;
pub mod loader;
pub mod model;
pub mod render;

pub use access::{JsonPatchAccess, PatchAccess};
pub use loader::SpecHandle;
pub use model::{
    ControlKind, ControlSpec, EnableCond, EnumOption, PluginUiSpec, RowSpec, SectionSpec, TabSpec,
};
pub use render::{render_layout, render_spec, render_spec_inner, CustomTable};




/// Typed setters in `engine_src` that NO control in `spec_src` reaches.
///
/// The direction that matters. Per-editor tests already check that every
/// control names a real patch field, which catches an INVENTED knob; nothing
/// caught a MISSING one until a draft turned out to cover 26 of 34 lane
/// fields. So this walks the command enum instead and reports what the layout
/// forgot.
///
/// `skip` is each editor's explicit list of commands it deliberately leaves
/// hand-written (step grids, text fields, anything a `param:` cannot express).
/// Declaring them is the point: an omission has to be stated, not inferred from
/// a control's absence.
pub fn uncovered_setters(
    spec_src: &str,
    engine_src: &str,
    prefix: &str,
    skip: &[&str],
) -> Vec<String> {
    let mut missing = Vec::new();
    for line in engine_src.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix(prefix) else { continue };
        let Some(name) = rest.split(['(', ' ', '{', ',']).next() else { continue };
        if name.is_empty() || skip.contains(&name) { continue }
        // CamelCase -> snake_case. A digit run starting after a letter also
        // breaks: `SetStrings16` addresses `strings_16`, and without this the
        // guard looked for `strings16`, found nothing, and reported three
        // perfectly-covered footings as missing.
        let mut snake = String::new();
        let chars: Vec<char> = name.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let starts_digit_run = c.is_ascii_digit()
                && i > 0
                && chars[i - 1].is_ascii_alphabetic();
            if (c.is_uppercase() || starts_digit_run) && i > 0 { snake.push('_'); }
            snake.push(c.to_ascii_lowercase());
        }
        if !covered(spec_src, &snake) { missing.push(snake); }
    }
    missing.sort();
    missing.dedup();
    missing
}

/// Every LEAF path of a serde value, in the dotted form the layouts address:
/// `choir.vowel`, `layers.3.level`, `voice_levels.0`.
///
/// Shared by the coverage guard (which asks what no control reaches) and by
/// editors fanning a loaded patch out one typed command per field. Written once
/// because the two must agree on what a "field" is: a hand-written fan-out that
/// walked only the TOP-level keys sent 7 commands and silently dropped 32
/// per-voice ones, which looks exactly like a preset that half-loaded.
pub fn leaf_paths(v: &serde_json::Value) -> Vec<String> {
    leaf_paths_with(v, LeafWalk::INDEXED)
}

/// How a walk names an array element, and what it does with a `null`.
///
/// Use [`LeafWalk::INDEXED`] to address a fixed-shape patch, and
/// [`LeafWalk::COLLAPSED`] to address a document whose array lengths are chosen
/// at runtime.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LeafWalk {
    /// Name every array element `*` instead of its index, so one path addresses
    /// the whole column: `tracks.*.volume`. Such a path is emitted once per
    /// element, in walk order, never deduplicated.
    pub collapse_arrays: bool,
    /// Emit a `null` as a leaf instead of skipping it, so an `Option` that is
    /// `None` still appears as a field of the document.
    pub keep_nulls: bool,
}

impl LeafWalk {
    /// `layers.3.level`, and a `null` is not a field.
    pub const INDEXED: Self = Self { collapse_arrays: false, keep_nulls: false };
    /// `tracks.*.volume`, nulls included.
    pub const COLLAPSED: Self = Self { collapse_arrays: true, keep_nulls: true };
}

/// [`leaf_paths`], with the array naming and the null handling chosen.
pub fn leaf_paths_with(v: &serde_json::Value, how: LeafWalk) -> Vec<String> {
    fn walk(v: &serde_json::Value, prefix: &str, how: LeafWalk, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, sub) in m {
                    let p = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                    walk(sub, &p, how, out);
                }
            }
            serde_json::Value::Array(a) => {
                for (i, sub) in a.iter().enumerate() {
                    let p = if how.collapse_arrays {
                        format!("{prefix}.*")
                    } else {
                        format!("{prefix}.{i}")
                    };
                    walk(sub, &p, how, out);
                }
            }
            serde_json::Value::Null => {
                if how.keep_nulls { out.push(prefix.to_string()); }
            }
            _ => out.push(prefix.to_string()),
        }
    }
    let mut out = Vec::new();
    walk(v, "", how, &mut out);
    out
}

/// Patch fields that NO control in `spec_src` names.
///
/// The strongest form of the "did the conversion lose a control?" question, and
/// the right one for a layout that addresses its patch BY PATH. `uncovered_setters`
/// walks the command enum, which only works when there is one command per field;
/// an engine with a single `SetParam { param, value }` has nothing to walk. This
/// walks the PATCH instead, so a whole group of knobs cannot go missing quietly.
///
/// `skip` is the editor's explicit list of path prefixes it deliberately leaves
/// hand-written (step grids, matrices, a master strip that lives above the tabs).
/// Declaring them is the point: an omission has to be stated.
pub fn uncovered_paths(spec_src: &str, sample: &serde_json::Value, skip: &[&str]) -> Vec<String> {
    leaf_paths(sample)
        .into_iter()
        .filter(|p| !skip.iter().any(|s| p == s || p.starts_with(&format!("{s}."))))
        .filter(|p| !spec_src.contains(&format!("\"{p}\"")))
        .collect()
}

/// Is `snake` (a command name, snake-cased) reachable from some spec path?
///
/// A command rarely spells its field exactly. It may address an ARRAY
/// (`SetOpLevel` -> `op_level.0`), a SUB-STRUCT (`SetFxAmount` -> `fx.amount`),
/// or an array OF sub-structs (`SetOpEnvA` -> `op_env.0.attack`). Rather than
/// force every spec to flatten its paths, this walks progressively shorter
/// prefixes of the command name and accepts a match on any of them.
///
/// Deliberately lenient about WHICH control, strict about whether the area is
/// addressed at all. The failure it exists to catch is a whole group of
/// controls going missing, which is what actually happened; a mislabelled knob
/// is caught by the per-editor param test instead.
fn covered(spec_src: &str, snake: &str) -> bool {
    let mut parts: Vec<&str> = snake.split('_').collect();
    while !parts.is_empty() {
        let joined = parts.join("_");
        let dotted = joined.replacen('_', ".", 1);
        for cand in [joined.as_str(), dotted.as_str()] {
            if spec_src.contains(&format!("\"{cand}\""))
                || spec_src.contains(&format!("\"{cand}."))
                || spec_src.contains(&format!(".{cand}\""))
                || spec_src.contains(&format!(".{cand}."))
            {
                return true;
            }
        }
        parts.pop();
    }
    false
}

/// Does `path` resolve in `root`? Mirrors `JsonPatchAccess::field`, including
/// numeric segments indexing arrays, so validation accepts exactly the paths
/// the renderer can actually reach.
fn path_exists(root: &serde_json::Value, path: &str) -> bool {
    let mut cur = root;
    for seg in path.split('.') {
        let next = match seg.parse::<usize>() {
            Ok(i) => cur.get(i),
            Err(_) => cur.get(seg),
        };
        match next {
            Some(v) => cur = v,
            None => return false,
        }
    }
    true
}

/// Validate a spec against a sample patch value (from
/// `serde_json::to_value(&Patch::default())`): every non-Custom control's `param`
/// must exist with a compatible shape. Returns human-readable problems (empty =
/// OK). Per-plugin tests call this to catch spec/patch drift at CI, not runtime.
pub fn validate(spec: &PluginUiSpec, sample: &serde_json::Value) -> Vec<String> {
    let mut errs = Vec::new();
    if !sample.is_object() {
        errs.push("sample patch is not a JSON object".to_string());
        return errs;
    }
    for tab in &spec.tabs {
        for section in &tab.sections {
            for row in &section.rows {
                for c in row {
                    match &c.kind {
                        ControlKind::Custom { .. } | ControlKind::Label { .. } => {}
                        ControlKind::Adsr => match sample.get(&c.param) {
                            Some(v) if v.is_object() => {}
                            Some(_) => errs.push(format!("Adsr '{}' is not a nested object", c.param)),
                            None => errs.push(format!("Adsr param '{}' missing from patch", c.param)),
                        },
                        _ => {
                            if c.param.is_empty() {
                                errs.push(format!("control '{}' has no param", c.label));
                            } else if !path_exists(sample, &c.param) {
                                errs.push(format!(
                                    "param '{}' (control '{}') missing from patch",
                                    c.param, c.label
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two walks differ only in array naming and null handling.
    ///
    /// A collapsed path is emitted once per element so a coverage table can be
    /// checked against the count, and a `None` appears as a field so a control
    /// table cannot omit it.
    #[test]
    fn a_collapsed_walk_keeps_the_count_and_the_absences() {
        let v = serde_json::json!({
            "tracks": [ { "volume": 0.8, "group": null }, { "volume": 0.2, "group": 1 } ],
            "bpm": 128.0,
        });

        let indexed = leaf_paths(&v);
        assert_eq!(indexed, vec!["bpm", "tracks.0.volume", "tracks.1.group", "tracks.1.volume"],
            "the indexed walk addresses elements by index and drops nulls");

        let collapsed = leaf_paths_with(&v, LeafWalk::COLLAPSED);
        assert_eq!(collapsed,
            vec!["bpm", "tracks.*.group", "tracks.*.volume", "tracks.*.group", "tracks.*.volume"],
            "the collapsed walk names every element `*`, emits nulls, and repeats");

        assert_eq!(collapsed.iter().filter(|p| *p == "tracks.*.volume").count(), 2,
            "a collapsed path carries one entry per element");
    }

    /// A spec must have no way to express "hide this when...". The house rule
    /// is that enabling a feature greys its params, never shows or hides them,
    /// because a layout that reflows under the cursor is worse than a dim knob.
    ///
    /// This is a source-level check on purpose. The rule is only structural for
    /// as long as `EnableCond` remains the ONLY conditional in the model, and
    /// the tempting next commit is a `visible_when` twin that quietly undoes it.
    #[test]
    fn the_spec_vocabulary_cannot_express_hiding() {
        let src = include_str!("model.rs");
        for banned in ["visible_when", "hidden_when", "show_when", "pub visible"] {
            assert!(
                !src.contains(banned),
                "`{banned}` appeared in the spec model: conditional VISIBILITY is \
                 not allowed, only EnableCond (which greys out)",
            );
        }
    }

}
