//! The plugin's saved state, written and read with no host.
//!
//! The state a host hands `set_state` is nice-plug's `PluginState` as
//! uncompressed JSON: `version`, `params` with every host parameter's
//! plain value under its stable id, and `fields` with each persisted key
//! (`patch`, `editor-state`) as a JSON string. With the `preset` parameter
//! above zero the plugin loads the `patch` field as it is and the other
//! host parameters only mirror it; at zero (Init) it builds its patch from
//! the host parameters. A VST3 host hands the JSON as the whole stream; a
//! CLAP host's stream is the JSON's length as a little-endian `u64`, then
//! the JSON.

use std::collections::BTreeMap;
use std::sync::Arc;

use archet::patch::ArchetPatch;
use nice_plug::params::InternalParamMut;
use nice_plug::plugin::ParamValue as StateValue;
use nice_plug::prelude::*;
use phonix_plugin::vstpreset::ParamValue;

use crate::{map_patch_to_nih_params, ArchetParams, ArchetPlugin};

/// The preset parameter's value for `name`: its place in the bank after
/// Init, or the bank's first preset when the bank has no such name.
fn preset_index(bank: &[ArchetPatch], name: &str) -> i32 {
    bank.iter().position(|p| p.name == name).map_or(1, |i| i as i32 + 1)
}

/// The parameters a fresh instance holds with `patch` loaded: the preset
/// parameter on the bank's preset of the same name, every other host
/// parameter on the patch's value.
fn params_for(patch: &ArchetPatch) -> ArchetParams {
    let bank = ArchetPatch::factory_presets();
    let mut names = vec![String::from("Init")];
    names.extend(bank.iter().map(|p| p.name.clone()));
    let params = ArchetParams::new(bank.len(), Arc::new(names));
    if let Ok(mut held) = params.patch_state.write() {
        *held = patch.clone();
    }
    let map: BTreeMap<String, ParamPtr> = params.param_map().into_iter().map(|(id, p, _)| (id, p)).collect();
    let mut values = map_patch_to_nih_params(patch);
    values.push(("preset", ParamValue::I32(preset_index(&bank, &patch.name))));
    for (id, v) in values {
        let Some(ptr) = map.get(id) else { continue };
        // SAFETY: every pointer is into `params`, alive for this loop.
        unsafe {
            match (ptr, v) {
                (ParamPtr::FloatParam(p), ParamValue::F32(x)) => (**p)._internal_set_plain_value(x),
                (ParamPtr::IntParam(p), ParamValue::I32(x)) => (**p)._internal_set_plain_value(x),
                _ => false,
            };
        }
    }
    params
}

/// The state bytes that load `patch`.
pub fn state_of(patch: &ArchetPatch) -> Vec<u8> {
    let params = params_for(patch);
    let mut values = BTreeMap::new();
    for (id, ptr, _) in params.param_map() {
        // SAFETY: every pointer is into `params`, alive for this loop.
        let v = unsafe {
            match ptr {
                ParamPtr::FloatParam(p) => StateValue::F32((*p).unmodulated_plain_value()),
                ParamPtr::IntParam(p) => StateValue::I32((*p).unmodulated_plain_value()),
                ParamPtr::BoolParam(p) => StateValue::Bool((*p).unmodulated_plain_value()),
                ParamPtr::EnumParam(p) => match (*p).unmodulated_plain_id() {
                    Some(s) => StateValue::String(s.to_owned()),
                    None => StateValue::I32((*p).unmodulated_plain_value()),
                },
            }
        };
        values.insert(id, v);
    }
    let state = PluginState { version: String::from(<ArchetPlugin as Plugin>::VERSION), params: values, fields: params.serialize_fields() };
    serde_json::to_vec(&state).unwrap_or_default()
}

/// `state` as a CLAP host's state stream: its length, then its bytes.
pub fn clap_stream(state: &[u8]) -> Vec<u8> {
    let mut out = (state.len() as u64).to_le_bytes().to_vec();
    out.extend_from_slice(state);
    out
}

/// The JSON of a state, from a VST3 stream or a CLAP stream.
fn json_of(state: &[u8]) -> &[u8] {
    match state.split_first_chunk::<8>() {
        Some((len, rest)) if u64::from_le_bytes(*len) == rest.len() as u64 => rest,
        _ => state,
    }
}

/// The patch a fresh instance plays after loading `state`: the parameters
/// and fields set as the wrapper sets them, then the persisted patch when
/// a preset is selected, or the patch Init builds from the parameters.
pub fn patch_of(state: &[u8]) -> Result<ArchetPatch, String> {
    let state: PluginState = serde_json::from_slice(json_of(state)).map_err(|e| format!("not an Archet state: {e}"))?;
    let bank = ArchetPatch::factory_presets();
    let mut names = vec![String::from("Init")];
    names.extend(bank.iter().map(|p| p.name.clone()));
    let params = ArchetParams::new(bank.len(), Arc::new(names));
    let map: BTreeMap<String, ParamPtr> = params.param_map().into_iter().map(|(id, p, _)| (id, p)).collect();
    for (id, v) in &state.params {
        let ptr = map.get(id).ok_or_else(|| format!("no parameter {id}"))?;
        // SAFETY: every pointer is into `params`, alive for this loop.
        unsafe {
            match (ptr, v) {
                (ParamPtr::FloatParam(p), StateValue::F32(x)) => (**p)._internal_set_plain_value(*x),
                (ParamPtr::IntParam(p), StateValue::I32(x)) => (**p)._internal_set_plain_value(*x),
                (ParamPtr::BoolParam(p), StateValue::Bool(x)) => (**p)._internal_set_plain_value(*x),
                (ParamPtr::EnumParam(p), StateValue::I32(x)) => (**p)._internal_set_plain_value(*x),
                (ParamPtr::EnumParam(p), StateValue::String(s)) => (**p).set_from_id(s),
                _ => return Err(format!("parameter {id} has a value of the wrong kind")),
            };
        }
    }
    params.deserialize_fields(&state.fields);
    if params.preset.value() == 0 {
        return Ok(params.build_init_patch());
    }
    let patch = params.patch_state.read().map_err(|e| e.to_string())?.clone();
    Ok(patch)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state written from a preset saves the preset as it is, selects it
    /// in the preset parameter and loads to the same patch, for every
    /// preset of the bank.
    #[test]
    fn state_round_trips_every_preset() {
        let bank = ArchetPatch::factory_presets();
        for (i, p) in bank.iter().enumerate() {
            let bytes = state_of(p);
            let state: PluginState = serde_json::from_slice(&bytes).expect("the state is JSON");
            assert!(matches!(state.params.get("preset"), Some(StateValue::I32(n)) if *n == i as i32 + 1), "{}", p.name);
            assert!(state.fields.contains_key("editor-state"), "{}", p.name);
            let saved: ArchetPatch = serde_json::from_str(&state.fields["patch"]).expect("the patch field reads");
            assert_eq!(serde_json::to_value(&saved).unwrap(), serde_json::to_value(p).unwrap(), "{}", p.name);
            let got = patch_of(&bytes).expect("the state loads");
            assert_eq!(serde_json::to_value(&got).unwrap(), serde_json::to_value(p).unwrap(), "{}", p.name);
        }
    }

    /// A CLAP stream carries the state behind its length and loads the same.
    #[test]
    fn a_clap_stream_loads() {
        let p = ArchetPatch::factory_presets().remove(0);
        let json = state_of(&p);
        let stream = clap_stream(&json);
        assert_eq!(stream.len(), json.len() + 8);
        let (a, b) = (patch_of(&json).unwrap(), patch_of(&stream).unwrap());
        assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap());
    }

    /// A state that is not one is refused with a message.
    #[test]
    fn a_foreign_state_is_refused() {
        assert!(patch_of(b"not json").is_err());
    }
}
