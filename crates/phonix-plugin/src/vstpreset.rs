//! Generator for .vstpreset files compatible with Cubase MediaBay.
//!
//! VST3 preset file format:
//!   Header (48 bytes): "VST3" magic, version, class ID, chunk list offset
//!   Component state data (variable)
//!   Chunk list: "List", count, {id, offset, size} entries

use std::io::Write;
use std::path::{Path, PathBuf};

/// The 4-byte chunk ID for component state.
const CHUNK_COMP: &[u8; 4] = b"Comp";
/// The 4-byte chunk ID for meta info.
const CHUNK_META: &[u8; 4] = b"Info";
/// The 4-byte chunk list marker.
const CHUNK_LIST: &[u8; 4] = b"List";

/// Build the XML MetaInfo blob for a preset with category.
fn build_meta_xml(category: &str, preset_name: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <MetaInfo>\n\
         \t<Attribute id=\"MediaType\" value=\"VstPreset\" type=\"string\"/>\n\
         \t<Attribute id=\"PlugCategory\" value=\"{}\" type=\"string\"/>\n\
         \t<Attribute id=\"PlugName\" value=\"{}\" type=\"string\"/>\n\
         </MetaInfo>\n",
        category, preset_name
    ).into_bytes()
}

/// Write a .vstpreset file with optional category metadata.
///
/// - `class_id`: The 16-byte VST3 class ID (same as `Vst3Plugin::VST3_CLASS_ID`)
/// - `state_json`: The serialized nih-plug PluginState as JSON bytes
/// - `category`: Optional category for MediaBay
/// - `preset_name`: Display name
fn write_vstpreset(
    path: &Path,
    class_id: &[u8; 16],
    state_json: &[u8],
    category: Option<&str>,
    preset_name: &str,
) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    write_vstpreset_to(&mut f, class_id, state_json, category, preset_name)
}

/// Build a .vstpreset in memory. Same bytes as [`write_vstpreset`], for callers
/// that embed the preset somewhere other than a file — the DAWproject exporter
/// writes it straight into the project ZIP.
pub fn vstpreset_bytes(
    class_id: &[u8; 16],
    state_json: &[u8],
    category: Option<&str>,
    preset_name: &str,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(state_json.len() + 256);
    // Writing to a Vec never fails.
    let _ = write_vstpreset_to(&mut buf, class_id, state_json, category, preset_name);
    buf
}

fn write_vstpreset_to<W: Write>(
    f: &mut W,
    class_id: &[u8; 16],
    state_json: &[u8],
    category: Option<&str>,
    preset_name: &str,
) -> std::io::Result<()> {
    let class_id_hex = class_id_to_hex(class_id);
    let comp_offset: i64 = 48; // component state starts right after header
    let comp_size = state_json.len() as i64;

    let (meta_xml, meta_offset, meta_size, chunk_count) = if let Some(cat) = category {
        let xml = build_meta_xml(cat, preset_name);
        let offset = comp_offset + comp_size;
        let size = xml.len() as i64;
        (Some(xml), offset, size, 2i32)
    } else {
        (None, 0i64, 0i64, 1i32)
    };

    let chunk_list_offset: i64 = comp_offset + comp_size + meta_xml.as_ref().map(|x| x.len() as i64).unwrap_or(0);

    // Header (48 bytes)
    f.write_all(b"VST3")?;
    f.write_all(&1i32.to_le_bytes())?;
    f.write_all(&class_id_hex)?;
    f.write_all(&chunk_list_offset.to_le_bytes())?;

    // Component state data
    f.write_all(state_json)?;

    // Meta info (if present)
    if let Some(ref xml) = meta_xml {
        f.write_all(xml)?;
    }

    // Chunk list
    f.write_all(CHUNK_LIST)?;
    f.write_all(&chunk_count.to_le_bytes())?;
    // Entry 1: Comp
    f.write_all(CHUNK_COMP)?;
    f.write_all(&comp_offset.to_le_bytes())?;
    f.write_all(&comp_size.to_le_bytes())?;
    // Entry 2: Meta (if present)
    if meta_xml.is_some() {
        f.write_all(CHUNK_META)?;
        f.write_all(&meta_offset.to_le_bytes())?;
        f.write_all(&meta_size.to_le_bytes())?;
    }

    Ok(())
}

/// Convert a 16-byte class ID to 32-byte ASCII hex representation.
pub fn class_id_to_hex(id: &[u8; 16]) -> [u8; 32] {
    let mut hex = [0u8; 32];
    for (i, &byte) in id.iter().enumerate() {
        let hi = byte >> 4;
        let lo = byte & 0x0F;
        hex[i * 2] = if hi < 10 { b'0' + hi } else { b'A' + hi - 10 };
        hex[i * 2 + 1] = if lo < 10 { b'0' + lo } else { b'A' + lo - 10 };
    }
    hex
}

/// Serialize a nih-plug PluginState as JSON.
/// The format matches nih-plug's internal serialization:
/// ```json
/// { "version": "0.1.0", "params": { "id": {"f32": val}, ... }, "fields": {} }
/// ```
pub fn make_state_json(
    version: &str,
    params: &[(&str, ParamValue)],
) -> Vec<u8> {
    make_state_json_with_fields(version, params, &[])
}

/// Same, plus nih-plug's `#[persist]` fields. Each field value is the field's
/// own JSON, which nih-plug stores as an escaped *string* inside `fields` — so
/// `("patch", "{\"cutoff\":0.5}")` lands as `"fields":{"patch":"{\"cutoff\":0.5}"}`.
///
/// This is what carries a full engine patch into a preset: the `params` map only
/// holds the handful of host-automatable knobs, while the plugin's real state
/// lives in these fields.
pub fn make_state_json_with_fields(
    version: &str,
    params: &[(&str, ParamValue)],
    fields: &[(&str, &str)],
) -> Vec<u8> {
    use std::fmt::Write as FmtWrite;
    let mut json = String::with_capacity(1024);
    json.push_str("{\"version\":\"");
    json.push_str(version);
    json.push_str("\",\"params\":{");
    for (i, (id, val)) in params.iter().enumerate() {
        if i > 0 { json.push(','); }
        let _ = write!(json, "\"{}\":{}", id, val.to_json());
    }
    json.push_str("},\"fields\":{");
    for (i, (key, raw_json)) in fields.iter().enumerate() {
        if i > 0 { json.push(','); }
        // Both the key and the embedded JSON go through serde so quotes,
        // backslashes and control characters are escaped correctly.
        let k = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into());
        let v = serde_json::to_string(raw_json).unwrap_or_else(|_| "\"\"".into());
        let _ = write!(json, "{k}:{v}");
    }
    json.push_str("}}");
    json.into_bytes()
}

/// A parameter value in nih-plug's serialization format.
#[derive(Clone, Debug)]
pub enum ParamValue {
    F32(f32),
    I32(i32),
    Bool(bool),
}

impl ParamValue {
    fn to_json(&self) -> String {
        match self {
            ParamValue::F32(v) => {
                if v.is_finite() {
                    format!("{{\"f32\":{}}}", v)
                } else {
                    "{\"f32\":0.0}".into()
                }
            }
            ParamValue::I32(v) => format!("{{\"i32\":{}}}", v),
            ParamValue::Bool(v) => format!("{{\"bool\":{}}}", v),
        }
    }
}

/// Get the VST3 preset directory for a given vendor/plugin.
/// Returns: `C:\Users\<user>\Documents\VST3 Presets\<vendor>\<plugin>\`
pub fn preset_dir(vendor: &str, plugin_name: &str) -> PathBuf {
    preset_dir_for(vendor, plugin_name)
}

/// Generate .vstpreset files for a whole factory bank, carrying each preset as
/// the engine's own patch rather than a hand-picked subset of knobs.
///
/// This is what makes a bank browsable in Cubase MediaBay without writing a
/// parameter mapping per engine: the patch goes into the nih-plug `#[persist]`
/// field the plugin already restores (`persist_key`), so loading the preset
/// gives the complete sound, not the handful of parameters that happen to be
/// host-automatable. Name and category come from the `Preset` trait, so the
/// MediaBay tree matches the in-editor picker.
///
/// Existing files are left alone, so a user's edits survive.
pub fn generate_patch_presets<T>(
    vendor: &str,
    plugin_name: &str,
    class_id: &[u8; 16],
    version: &str,
    persist_key: &str,
    presets: &[T],
) -> std::io::Result<usize>
where
    T: serde::Serialize + crate::preset::Preset,
{
    let base = preset_dir(vendor, plugin_name);
    generate_patch_presets_in(&base, class_id, version, persist_key, presets)
}

/// Same, into an explicit directory. Split out so the format can be tested
/// without writing into the user's preset folder.
pub fn generate_patch_presets_in<T>(
    base: &Path,
    class_id: &[u8; 16],
    version: &str,
    persist_key: &str,
    presets: &[T],
) -> std::io::Result<usize>
where
    T: serde::Serialize + crate::preset::Preset,
{
    let base = base.to_path_buf();
    std::fs::create_dir_all(&base)?;

    let mut count = 0;
    for preset in presets {
        let category = preset.preset_category().unwrap_or("");
        let dir = if category.is_empty() {
            base.clone()
        } else {
            let d = base.join(sanitize_preset_component(category));
            std::fs::create_dir_all(&d)?;
            d
        };
        let name = preset.preset_name();
        let path = dir.join(format!("{}.vstpreset", sanitize_preset_component(name)));
        if path.exists() {
            continue;
        }
        let Ok(patch_json) = serde_json::to_string(preset) else { continue };
        let state = make_state_json_with_fields(version, &[], &[(persist_key, patch_json.as_str())]);
        write_vstpreset(
            &path,
            class_id,
            &state,
            (!category.is_empty()).then_some(category),
            name,
        )?;
        count += 1;
    }
    Ok(count)
}

fn sanitize_preset_component(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if out.trim().is_empty() { "Preset".into() } else { out }
}

/// Generate .vstpreset files for a set of factory presets.
///
/// - `vendor` / `plugin_name`: for the directory path
/// - `class_id`: VST3 class ID (16 bytes)
/// - `version`: plugin version string
/// - `presets`: list of (name, params) pairs
///
/// Creates subcategories as subdirectories if the name contains "/".
/// Skips files that already exist (won't overwrite user modifications).
pub fn generate_factory_presets(
    vendor: &str,
    plugin_name: &str,
    class_id: &[u8; 16],
    version: &str,
    presets: &[(&str, Vec<(&str, ParamValue)>)],
) -> std::io::Result<usize> {
    let base = preset_dir(vendor, plugin_name);
    std::fs::create_dir_all(&base)?;

    let mut count = 0;
    for (name, params) in presets {
        // Support "Category/Preset Name" → subdirectory
        let (subdir, filename) = if let Some(pos) = name.rfind('/') {
            let dir = &name[..pos];
            let file = &name[pos + 1..];
            (Some(dir), file)
        } else {
            (None, *name)
        };

        let dir = if let Some(sub) = subdir {
            let d = base.join(sub);
            std::fs::create_dir_all(&d)?;
            d
        } else {
            base.clone()
        };

        let safe_name: String = filename.chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let path = dir.join(format!("{}.vstpreset", safe_name));

        // Don't overwrite existing presets
        if path.exists() { continue; }

        // Extract category from "Category/Name" path
        let category = subdir;

        let state = make_state_json(version, params);
        write_vstpreset(&path, class_id, &state, category, filename)?;
        count += 1;
    }

    Ok(count)
}

/// Where a host looks for factory `.vstpreset` banks, per the VST3 locations.
///
/// Resolved by hand rather than through `dirs_next`, which is not a dependency.
///
/// This used to be the Windows path on every platform, with a literal
/// `C:\Users\Public\Documents` fallback. On Linux that is not an absolute path
/// but a single directory NAME containing backslashes, so every plugin
/// instantiation wrote its whole factory bank into a junk folder in whatever
/// the current directory happened to be. The Windows branch is unchanged, so
/// no MediaBay-indexed preset is orphaned.
pub fn preset_dir_for(vendor: &str, plugin_name: &str) -> PathBuf {
    let root = {
        #[cfg(target_os = "windows")]
        {
            std::env::var("USERPROFILE")
                .map(|p| PathBuf::from(p).join("Documents").join("VST3 Presets"))
                .unwrap_or_else(|_| PathBuf::from("C:\\Users\\Public\\Documents\\VST3 Presets"))
        }
        #[cfg(target_os = "macos")]
        {
            std::env::var("HOME")
                .map(|p| PathBuf::from(p).join("Library/Audio/Presets"))
                .unwrap_or_else(|_| PathBuf::from("/Library/Audio/Presets"))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            std::env::var("HOME")
                .map(|p| PathBuf::from(p).join(".vst3presets"))
                .unwrap_or_else(|_| std::env::temp_dir().join("vst3presets"))
        }
    };
    root.join(vendor).join(plugin_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::Preset;

    /// Slice the `Comp` chunk back out of a preset file — the same walk the
    /// DAWproject importer does, so this also guards that format.
    fn comp_chunk(bytes: &[u8]) -> Option<&[u8]> {
        if bytes.len() < 48 || &bytes[0..4] != b"VST3" {
            return None;
        }
        let list_off = i64::from_le_bytes(bytes[40..48].try_into().ok()?) as usize;
        if list_off + 8 > bytes.len() || &bytes[list_off..list_off + 4] != b"List" {
            return None;
        }
        let count = i32::from_le_bytes(bytes[list_off + 4..list_off + 8].try_into().ok()?).max(0) as usize;
        for i in 0..count {
            let e = list_off + 8 + i * 20;
            if e + 20 > bytes.len() || &bytes[e..e + 4] != b"Comp" {
                continue;
            }
            let off = i64::from_le_bytes(bytes[e + 4..e + 12].try_into().ok()?) as usize;
            let size = i64::from_le_bytes(bytes[e + 12..e + 20].try_into().ok()?) as usize;
            return bytes.get(off..off.checked_add(size)?);
        }
        None
    }

    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug, Clone)]
    struct FakePatch {
        name: String,
        category: String,
        cutoff: f32,
    }

    impl Preset for FakePatch {
        fn preset_name(&self) -> &str { &self.name }
        fn preset_category(&self) -> Option<&str> { Some(&self.category) }
    }

    /// A generated bank has to be readable back as the patch it was made from —
    /// that is the whole point of carrying it in a `#[persist]` field rather
    /// than a subset of automatable knobs.
    #[test]
    fn a_generated_bank_round_trips_to_the_patch() {
        let dir = std::env::temp_dir().join(format!("phonix-vstpreset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let bank = vec![
            FakePatch { name: "Bright Lead".into(), category: "Lead".into(), cutoff: 0.8 },
            FakePatch { name: "Deep/Slash".into(), category: "Bass".into(), cutoff: 0.2 },
        ];
        let n = generate_patch_presets_in(&dir, b"PxTestClassId001", "1.0", "patch", &bank)
            .expect("write bank");
        assert_eq!(n, 2);

        // Category becomes a subdirectory, and a name with a path separator
        // must not escape it.
        let f = dir.join("Lead").join("Bright Lead.vstpreset");
        assert!(f.exists(), "expected {}", f.display());
        assert!(dir.join("Bass").join("Deep_Slash.vstpreset").exists());

        let bytes = std::fs::read(&f).unwrap();
        let comp = comp_chunk(&bytes).expect("Comp chunk");
        let state: serde_json::Value = serde_json::from_slice(comp).unwrap();
        let field = state["fields"]["patch"].as_str().expect("patch field");
        let back: FakePatch = serde_json::from_str(field).unwrap();
        assert_eq!(back, bank[0]);

        // A second run must not overwrite what is already there.
        let again = generate_patch_presets_in(&dir, b"PxTestClassId001", "1.0", "patch", &bank)
            .expect("second run");
        assert_eq!(again, 0, "existing presets must be left alone");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
