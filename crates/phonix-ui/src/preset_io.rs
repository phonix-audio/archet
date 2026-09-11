//! Shared disk save/load for plugin patches.
//!
//! Every engine's patch is serde-serializable (that's how the session persists
//! engine state), so a per-plugin "save/load to disk" is just a file picker plus
//! JSON. These two helpers centralise that so each editor only needs a Save and
//! a Load button, instead of re-deriving the dialog + serialization wiring.
//!
//! Both dialogs default to a per-engine user-preset directory,
//! `<data_dir>/presets/<engine>/` under the `PresetHome` the caller names:
//! the sequencer's for every instrument it hosts, a plugin's own for a
//! plugin that stands alone.

use std::path::PathBuf;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Where user presets live on disk: the vendor and product the platform's
/// data directory is keyed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresetHome {
    pub organisation: &'static str,
    pub application: &'static str,
}

/// The sequencer's home, shared by every instrument it hosts.
pub const PHONIX: PresetHome = PresetHome { organisation: "Phonix", application: "phonix" };

/// Path-safe folder name for an engine: keep ASCII alphanumerics, fold anything
/// else (spaces, hyphens, separators) to `_` so no path component can escape.
fn preset_slug(engine: &str) -> String {
    engine
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// Per-engine user-preset directory, created on demand. `None` only if the
/// platform has no resolvable data directory (headless/sandboxed), in which
/// case the dialogs simply fall back to the picker's own default location.
fn user_preset_dir(home: PresetHome, engine: &str) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", home.organisation, home.application)?;
    let dir = dirs.data_dir().join("presets").join(preset_slug(engine));
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// Open a save dialog and write `patch` as pretty JSON. `engine` names the
/// instrument (dialog title + preset folder), `default_name` seeds the filename.
/// Returns true when a file was actually written.
pub fn save_patch_to_disk<P: Serialize>(home: PresetHome, patch: &P, engine: &str, default_name: &str) -> bool {
    let mut dialog = rfd::FileDialog::new()
        .set_title(format!("Save {engine} Patch"))
        .add_filter("JSON Patch", &["json"])
        .set_file_name(format!("{default_name}.json"));
    if let Some(dir) = user_preset_dir(home, engine) {
        dialog = dialog.set_directory(dir);
    }
    if let Some(path) = dialog.save_file() {
        if let Ok(json) = serde_json::to_string_pretty(patch) {
            return std::fs::write(path, json).is_ok();
        }
    }
    false
}

/// Open a load dialog, read and deserialize a patch. Returns `None` if the user
/// cancelled or the file could not be read/parsed as `P`.
pub fn load_patch_from_disk<P: DeserializeOwned>(home: PresetHome, engine: &str) -> Option<P> {
    let mut dialog = rfd::FileDialog::new()
        .set_title(format!("Load {engine} Patch"))
        .add_filter("JSON Patch", &["json"]);
    if let Some(dir) = user_preset_dir(home, engine) {
        dialog = dialog.set_directory(dir);
    }
    let path = dialog.pick_file()?;
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_slug_is_path_safe() {
        assert_eq!(preset_slug("Big Kick"), "Big_Kick");
        assert_eq!(preset_slug("VP330"), "VP330");
        assert_eq!(preset_slug("TB-303"), "TB_303");
        // No path separator can survive into a folder name.
        let s = preset_slug("a/b\\c..d");
        assert!(!s.contains('/') && !s.contains('\\') && !s.contains('.'), "got {s}");
    }

    #[test]
    fn user_preset_dir_is_per_engine_under_presets() {
        // ProjectDirs may be absent in a sandbox; only assert when it resolves.
        if let Some(dir) = user_preset_dir(PHONIX, "Big Kick") {
            assert!(dir.ends_with("Big_Kick"), "leaf must be the engine slug: {dir:?}");
            assert!(dir.parent().map_or(false, |p| p.ends_with("presets")),
                "parent must be the shared presets/ folder: {dir:?}");
        }
    }
}

/// The shared Save / Load pair, for an editor whose chrome is hand-drawn.
///
/// `plugin_chrome_panel` already offers these, but several editors keep a
/// bespoke top bar and so had NO on-disk user presets at all: the factory bank
/// and nothing else. Others hand-rolled their own `rfd` dialogs, and their files
/// did not land in the same per-engine directory as everyone else's.
///
/// Returns the loaded patch when the user picked one.
pub fn disk_preset_buttons<P>(
    ui: &mut egui::Ui,
    home: PresetHome,
    engine: &str,
    current: &P,
    default_name: &str,
) -> Option<P>
where
    P: Serialize + DeserializeOwned,
{
    let mut loaded = None;
    if ui.add(egui::Button::new(egui::RichText::new("Save").size(10.0))
        .min_size(egui::vec2(38.0, 20.0)))
        .on_hover_text("Save this patch to disk")
        .clicked()
    {
        save_patch_to_disk(home, current, engine, default_name);
    }
    if ui.add(egui::Button::new(egui::RichText::new("Load").size(10.0))
        .min_size(egui::vec2(38.0, 20.0)))
        .on_hover_text("Load a patch from disk")
        .clicked()
    {
        loaded = load_patch_from_disk::<P>(home, engine);
    }
    loaded
}
