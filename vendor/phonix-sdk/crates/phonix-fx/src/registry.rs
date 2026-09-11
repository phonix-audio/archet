//! The kinds of effect a build knows, by name.

use crate::effect::Effect;
use crate::spec::EffectSpec;

/// How to describe and how to build one kind.
#[derive(Clone, Copy)]
pub struct Entry {
    pub spec: &'static EffectSpec,
    pub build: fn(sample_rate: f32) -> Box<dyn Effect>,
}

/// A visible list of kinds. `builtin()` holds every effect this build was
/// compiled with; `register` adds one a repository provides itself.
#[derive(Clone, Default)]
pub struct Registry {
    entries: Vec<Entry>,
}

impl Registry {
    pub fn new() -> Self {
        Registry { entries: Vec::new() }
    }

    /// Every kind compiled in. Empty in a contract-only build.
    pub fn builtin() -> Self {
        Registry { entries: crate::effects::builtin() }
    }

    /// Adds `entry`, replacing an entry of the same kind.
    pub fn register(&mut self, entry: Entry) -> &mut Self {
        match self.entries.iter_mut().find(|e| e.spec.kind == entry.spec.kind) {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
        self
    }

    pub fn get(&self, kind: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.spec.kind == kind)
    }

    pub fn contains(&self, kind: &str) -> bool {
        self.get(kind).is_some()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn kinds(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.iter().map(|e| e.spec.kind)
    }

    /// The same kinds minus `kind`: what a rack applies its sub-chains
    /// against, so nesting stops at one level.
    pub fn without(&self, kind: &str) -> Registry {
        Registry { entries: self.entries.iter().copied().filter(|e| e.spec.kind != kind).collect() }
    }

    pub fn build(&self, kind: &str, sample_rate: f32) -> Option<Box<dyn Effect>> {
        self.get(kind).map(|e| (e.build)(sample_rate))
    }
}
