//! Dynamic parameter access by field name — the reflection Rust lacks, via serde.
//!
//! The renderer never sees a concrete `Patch`; it sees a `dyn PatchAccess`. The
//! `JsonPatchAccess` implementation holds the patch as a live `serde_json::Value`
//! object for the duration of a frame: reads/writes address fields by name, and
//! on change the editor deserializes back to the concrete `Patch` and sends its
//! existing `LoadPatch`. GUI-thread only; the audio/RT path is untouched.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

pub trait PatchAccess {
    fn get_f32(&self, name: &str) -> Option<f32>;
    fn get_bool(&self, name: &str) -> Option<bool>;
    fn get_int(&self, name: &str) -> Option<i64>;
    /// Fieldless enum -> its serialized variant name.
    fn get_enum(&self, name: &str) -> Option<String>;
    /// A field inside a nested object (e.g. an `EnvParams` ADSR block).
    fn get_nested_f32(&self, obj: &str, field: &str) -> Option<f32>;

    fn set_f32(&mut self, name: &str, v: f32);
    fn set_bool(&mut self, name: &str, v: bool);
    fn set_int(&mut self, name: &str, v: i64);
    fn set_enum(&mut self, name: &str, variant: &str);
    fn set_nested_f32(&mut self, obj: &str, field: &str, v: f32);

    /// Raw access for `Custom` controls (arrays / Option / nested). Call
    /// [`PatchAccess::mark_dirty`] after writing through this.
    fn value_mut(&mut self) -> &mut Value;
    fn mark_dirty(&mut self);
    fn take_dirty(&mut self) -> bool;

    /// Names of the fields edited since the last call, and clear the list.
    ///
    /// Lets an editor send a TYPED per-parameter command instead of shipping the
    /// whole patch on every edit. That matters because most engines' `LoadPatch`
    /// deliberately disturbs voices — releasing notes, re-seeding, rebuilding
    /// resonators — which is right for switching preset but would cut the sound
    /// on every knob turn if used for live edits.
    fn take_changed(&mut self) -> Vec<String>;
}

/// serde_json-backed [`PatchAccess`]. Build once per frame from `&Patch`, edit
/// by name, then `into_patch()` on change.
pub struct JsonPatchAccess {
    root: Value,
    dirty: bool,
    /// Fields edited this frame, so an editor can send typed per-parameter
    /// commands rather than a whole-patch load.
    changed: Vec<String>,
}

impl JsonPatchAccess {
    pub fn new<P: Serialize>(patch: &P) -> Self {
        Self {
            root: serde_json::to_value(patch).unwrap_or(Value::Null),
            dirty: false,
            changed: Vec::new(),
        }
    }

    /// Deserialize the (possibly edited) value back into the concrete patch.
    pub fn into_patch<P: DeserializeOwned>(self) -> Option<P> {
        serde_json::from_value(self.root).ok()
    }

    /// Same, without consuming the access.
    ///
    /// An editor sending one typed command per changed field needs to read the
    /// edited patch once per field, so it cannot consume the access to do it.
    pub fn patch_view<P: DeserializeOwned>(&self) -> Option<P> {
        serde_json::from_value(self.root.clone()).ok()
    }

    /// Resolve a dotted path such as `layers.3.level`.
    ///
    /// A plain name is the common case and stays a single `get`. Paths exist
    /// because several patches keep an ARRAY of sub-structs (eight layers,
    /// three LFOs), and without indexing those editors would have to
    /// hand-write every one of their controls, which is exactly the duplication
    /// the declarative layer removes.
    fn field(&self, name: &str) -> Option<&Value> {
        if !name.contains('.') {
            return self.root.get(name);
        }
        let mut cur = &self.root;
        for seg in name.split('.') {
            cur = match seg.parse::<usize>() {
                Ok(i) => cur.get(i)?,
                Err(_) => cur.get(seg)?,
            };
        }
        Some(cur)
    }

    /// Mutable twin of `field`. Returns None if any segment is missing, so a
    /// mistyped path writes NOTHING rather than growing a stray object that
    /// would then fail to deserialize back into the patch.
    fn field_mut(&mut self, name: &str) -> Option<&mut Value> {
        if !name.contains('.') {
            return self.root.as_object_mut()?.get_mut(name);
        }
        let mut cur = &mut self.root;
        for seg in name.split('.') {
            cur = match seg.parse::<usize>() {
                Ok(i) => cur.get_mut(i)?,
                Err(_) => cur.get_mut(seg)?,
            };
        }
        Some(cur)
    }

    /// Write `v` at `name`, recording the change. Returns false when the path
    /// does not resolve.
    fn put(&mut self, name: &str, v: Value) -> bool {
        // A path has to be created when absent for a plain name (the original
        // behaviour: `insert`), but never for a nested one.
        if !name.contains('.') {
            if let Some(m) = self.root.as_object_mut() {
                m.insert(name.to_string(), v);
                self.dirty = true;
                self.changed.push(name.to_string());
                return true;
            }
            return false;
        }
        match self.field_mut(name) {
            Some(slot) => {
                *slot = v;
                self.dirty = true;
                self.changed.push(name.to_string());
                true
            }
            None => false,
        }
    }
}

impl PatchAccess for JsonPatchAccess {
    fn get_f32(&self, name: &str) -> Option<f32> {
        self.field(name)?.as_f64().map(|v| v as f32)
    }
    fn get_bool(&self, name: &str) -> Option<bool> {
        self.field(name)?.as_bool()
    }
    fn get_int(&self, name: &str) -> Option<i64> {
        self.field(name)?.as_i64()
    }
    fn get_enum(&self, name: &str) -> Option<String> {
        self.field(name)?.as_str().map(str::to_string)
    }
    fn get_nested_f32(&self, obj: &str, field: &str) -> Option<f32> {
        self.field(obj)?.get(field)?.as_f64().map(|v| v as f32)
    }

    fn set_f32(&mut self, name: &str, v: f32) {
        // Preserve integer-typed fields: writing a float (4.0) into a usize/u8
        // field breaks deserialization, so round-trip through i64 when the
        // current value is an integer.
        let was_int = self.field(name).map(|x| x.is_i64() || x.is_u64()).unwrap_or(false);
        let val = if was_int {
            serde_json::json!(v.round() as i64)
        } else {
            serde_json::json!(v)
        };
        self.put(name, val);
    }
    fn set_bool(&mut self, name: &str, v: bool) {
        self.put(name, Value::Bool(v));
    }
    fn set_int(&mut self, name: &str, v: i64) {
        self.put(name, serde_json::json!(v));
    }
    fn set_enum(&mut self, name: &str, variant: &str) {
        self.put(name, Value::String(variant.to_string()));
    }
    fn set_nested_f32(&mut self, obj: &str, field: &str, v: f32) {
        if let Some(o) = self.root.get_mut(obj).and_then(Value::as_object_mut) {
            o.insert(field.to_string(), serde_json::json!(v));
            self.dirty = true;
            self.changed.push(obj.to_string());
        }
    }

    fn value_mut(&mut self) -> &mut Value {
        &mut self.root
    }
    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
    fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }
    fn take_changed(&mut self) -> Vec<String> {
        let mut v = std::mem::take(&mut self.changed);
        v.sort();
        v.dedup();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Demo { cutoff: f32, poly: i64, on: bool }

    /// An editor must be able to learn WHICH fields changed, so it can send a
    /// typed per-parameter command instead of a whole-patch load. Most engines'
    /// LoadPatch deliberately disturbs voices (releasing notes, re-seeding),
    /// which is right for a preset switch but would cut the sound on every knob
    /// turn during a live edit.
    #[test]
    fn changed_fields_are_reported_then_cleared() {
        let mut a = JsonPatchAccess::new(&Demo { cutoff: 0.2, poly: 4, on: false });
        a.set_f32("cutoff", 0.8);
        a.set_bool("on", true);
        // Editing the same field twice reports it once.
        a.set_f32("cutoff", 0.9);

        let changed = a.take_changed();
        assert_eq!(changed, vec!["cutoff".to_string(), "on".to_string()]);
        assert!(a.take_changed().is_empty(), "the list must clear once taken");
    }

    #[test]
    fn an_untouched_access_reports_nothing() {
        let mut a = JsonPatchAccess::new(&Demo { cutoff: 0.2, poly: 4, on: false });
        assert!(a.take_changed().is_empty());
        assert!(!a.take_dirty());
    }

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Default, Clone)]
    struct Layer { level: f32, on: bool, mode: i64 }
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Default)]
    struct Rig { layers: Vec<Layer>, master: f32 }

    /// An indexed path must read and write the RIGHT element, and report itself
    /// verbatim so the editor can parse the index back out.
    #[test]
    fn an_indexed_path_reaches_one_array_element() {
        let rig = Rig { layers: vec![Layer::default(); 3], master: 0.5 };
        let mut a = JsonPatchAccess::new(&rig);

        a.set_f32("layers.1.level", 0.75);
        a.set_bool("layers.2.on", true);
        a.set_int("layers.0.mode", 4);

        assert_eq!(a.get_f32("layers.1.level"), Some(0.75));
        assert_eq!(a.get_bool("layers.2.on"), Some(true));
        assert_eq!(a.get_int("layers.0.mode"), Some(4));
        // Neighbours untouched.
        assert_eq!(a.get_f32("layers.0.level"), Some(0.0));
        assert_eq!(a.get_bool("layers.1.on"), Some(false));

        let changed = PatchAccess::take_changed(&mut a);
        assert!(changed.contains(&"layers.1.level".to_string()));

        // And it still deserializes: writing through a path must not corrupt
        // the shape.
        let back: Rig = a.patch_view().expect("patch must still deserialize");
        assert_eq!(back.layers[1].level, 0.75);
        assert_eq!(back.master, 0.5);
    }

    /// A path that does not resolve must write NOTHING.
    ///
    /// The old plain-name setter inserted a missing key, which is right at the
    /// root. Doing that mid-path would graft a stray object onto the patch and
    /// the next deserialize would fail, turning a spec typo into a dead editor.
    #[test]
    fn a_bad_path_writes_nothing() {
        let rig = Rig { layers: vec![Layer::default(); 2], master: 0.5 };
        let mut a = JsonPatchAccess::new(&rig);
        a.set_f32("layers.9.level", 0.9);      // out of range
        a.set_f32("layers.0.nosuch", 0.9);     // no such field
        a.set_f32("nosuch.0.level", 0.9);      // no such array
        assert!(PatchAccess::take_changed(&mut a).is_empty(),
            "an unresolvable path must not report a change");
        let back: Rig = a.patch_view().expect("patch must still deserialize");
        assert_eq!(back.layers.len(), 2);
    }
}
