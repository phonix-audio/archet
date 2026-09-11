//! Load a `.ron` layout spec, with dev-time hot-reload by mtime polling and a
//! baked fallback for release. Never crashes: a missing or unparseable file
//! keeps the last-good spec and surfaces an error string for the editor to show.

use super::model::PluginUiSpec;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

enum SpecSource {
    /// Dev: watch a repo file and reload on change.
    Disk { path: PathBuf, mtime: Option<SystemTime> },
    /// Release (or missing file): use the compiled-in default.
    Baked,
}

pub struct SpecHandle {
    spec: PluginUiSpec,
    source: SpecSource,
    error: Option<String>,
}

impl SpecHandle {
    /// `plugin` is the file stem (`"voice"` -> `ui_specs/voice.ron`); `baked` is the
    /// `include_str!` of that same file (the shipped default).
    pub fn load(plugin: &str, baked: &'static str) -> Self {
        let baked_spec = ron::from_str::<PluginUiSpec>(baked).unwrap_or_else(|e| {
            // The baked spec is include_str! from the repo and is validated by a
            // per-plugin test; if it ever fails at runtime, degrade instead of crash.
            eprintln!("ui_spec: baked spec for {plugin} failed to parse: {e}");
            empty_spec(plugin)
        });

        // Dev override: <repo>/ui_specs/<plugin>.ron, hot-reloaded.
        if let Some(path) = disk_path(plugin) {
            if path.exists() {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    match ron::from_str::<PluginUiSpec>(&text) {
                        Ok(spec) => {
                            let mtime = mtime_of(&path);
                            return Self { spec, source: SpecSource::Disk { path, mtime }, error: None };
                        }
                        Err(e) => {
                            // File present but broken: watch it, use baked meanwhile.
                            let mtime = mtime_of(&path);
                            return Self {
                                spec: baked_spec,
                                source: SpecSource::Disk { path, mtime },
                                error: Some(format!("ui_spec parse error: {e}")),
                            };
                        }
                    }
                }
            }
        }
        Self { spec: baked_spec, source: SpecSource::Baked, error: None }
    }

    /// Call once per frame (cheap: one `fs::metadata`). Reloads on mtime change.
    pub fn poll(&mut self) {
        let SpecSource::Disk { path, mtime } = &mut self.source else { return };
        let cur = mtime_of(path);
        if cur == *mtime {
            return;
        }
        *mtime = cur;
        match std::fs::read_to_string(&*path) {
            Ok(text) => match ron::from_str::<PluginUiSpec>(&text) {
                Ok(spec) => {
                    self.spec = spec;
                    self.error = None;
                }
                // Keep the last-good spec; show the error.
                Err(e) => self.error = Some(format!("ui_spec parse error: {e}")),
            },
            Err(e) => self.error = Some(format!("ui_spec read error: {e}")),
        }
    }

    pub fn spec(&self) -> &PluginUiSpec {
        &self.spec
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Repo path for the dev override, only in debug builds (release = baked, no
/// filesystem syscalls per frame).
///
/// `ui_specs/` sits at the repo root and this crate lives in `crates/phonix-ui`,
/// so the manifest dir has to climb two levels — the same `../../` the
/// `include_str!` of the baked defaults walks.
fn disk_path(plugin: &str) -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui_specs").join(format!("{plugin}.ron")))
    } else {
        None
    }
}

fn mtime_of(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn empty_spec(title: &str) -> PluginUiSpec {
    PluginUiSpec { title: title.to_string(), accent: [200, 200, 200], columns: Some(1),
                   knob_size: None, tabs: Vec::new() }
}

#[cfg(test)]
impl SpecHandle {
    /// Build a Disk-sourced handle from an arbitrary path (for the hot-reload test).
    fn from_disk_path_for_test(path: PathBuf) -> Self {
        let spec = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| ron::from_str(&t).ok())
            .unwrap_or_else(|| empty_spec("test"));
        let mtime = mtime_of(&path);
        Self { spec, source: SpecSource::Disk { path, mtime }, error: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_A: &str = r#"PluginUiSpec(title: "A", accent: (1,2,3), tabs: [])"#;
    const SPEC_B: &str = r#"PluginUiSpec(title: "B", accent: (4,5,6), tabs: [])"#;

    #[test]
    fn hot_reload_picks_up_a_file_edit() {
        let path = std::env::temp_dir().join(format!("phonix_uispec_test_{}.ron", std::process::id()));
        std::fs::write(&path, SPEC_A).unwrap();

        let mut h = SpecHandle::from_disk_path_for_test(path.clone());
        assert_eq!(h.spec().title, "A");

        // No change yet -> poll is a no-op.
        h.poll();
        assert_eq!(h.spec().title, "A");

        // Edit the file (ensure a distinct mtime), then poll: it reloads.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, SPEC_B).unwrap();
        h.poll();
        assert_eq!(h.spec().title, "B", "poll() should have hot-reloaded the edited spec");
        assert!(h.error().is_none());

        // A broken edit keeps the last-good spec and surfaces an error.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "not valid ron {{{").unwrap();
        h.poll();
        assert_eq!(h.spec().title, "B", "broken edit must keep the last-good spec");
        assert!(h.error().is_some());

        let _ = std::fs::remove_file(&path);
    }
}
