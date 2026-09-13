//! Running the chain a patch describes.
//!
//! The description lives in `archet::fx`, with the presets that carry it.
//! What is here is the half that needs a live `Chain`, which is the host's
//! and never the engine's.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use phonix_fx::{ApplyReport, Chain, ChainSpec, Registry};

/// The chain the audio thread runs, as the editor and the audio thread hand
/// it to each other: a value and a counter. The value is always written
/// before the counter moves, so a reader that sees a new number reads the
/// value that goes with it. Each side remembers the number it last saw.
pub struct FxLink {
    live: RwLock<ChainSpec>,
    rev: AtomicU64,
}

impl FxLink {
    pub fn new(spec: ChainSpec) -> Self {
        FxLink { live: RwLock::new(spec), rev: AtomicU64::new(0) }
    }

    pub fn rev(&self) -> u64 {
        self.rev.load(Ordering::Acquire)
    }

    /// Replaces the value without moving the counter: what a side that has
    /// already applied `spec` itself writes, so the other side finds it.
    pub fn seed(&self, spec: ChainSpec) {
        if let Ok(mut w) = self.live.write() {
            *w = spec;
        }
    }

    /// Writes `spec`, then moves the counter; returns the number the writer
    /// has now seen.
    pub fn publish(&self, spec: ChainSpec) -> u64 {
        if let Ok(mut w) = self.live.write() {
            *w = spec;
        }
        self.rev.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// The editor's side: the chain, when the counter moved since `seen`.
    /// May wait for the lock.
    pub fn adopt(&self, seen: &mut u64) -> Option<ChainSpec> {
        let rev = self.rev();
        if rev == *seen {
            return None;
        }
        let spec = self.live.read().ok()?.clone();
        *seen = rev;
        Some(spec)
    }

    /// The audio thread's side: runs `f` on the chain when the counter moved
    /// since `seen`, and never waits for the lock; a block missed is picked
    /// up on the next one. Returns whether `f` ran.
    pub fn apply_if_new(&self, seen: &mut u64, f: impl FnOnce(&ChainSpec)) -> bool {
        let rev = self.rev();
        if rev == *seen {
            return false;
        }
        let Ok(spec) = self.live.try_read() else { return false };
        f(&spec);
        *seen = rev;
        true
    }
}

/// The three kinds this build ships, and nothing else.
pub fn registry() -> Registry {
    Registry::builtin()
}

/// Build `spec` into `chain`, replacing whatever it held. Allocates: called
/// on a preset change and on an edit, never per block.
pub fn apply(chain: &mut Chain, spec: &ChainSpec) -> ApplyReport {
    chain.apply(spec, &registry())
}

/// Empty the chain. `Init` is the absence of a factory preset, so it is the
/// absence of the chain one carries.
pub fn disengage(chain: &mut Chain) {
    chain.apply(&ChainSpec::default(), &registry());
}

#[cfg(test)]
mod tests {
    use super::*;
    use archet::fx::{chain as recipe, CHAMBER, FX_SLOTS};

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 256;

    fn pushed(mut spec: ChainSpec) -> ChainSpec {
        spec.slots[1].set("size", 0.9_f32);
        spec
    }

    fn size(chain: &Chain) -> f32 {
        chain.param(chain.param_ref(1, "size").unwrap()).unwrap().as_f32()
    }

    /// The editor edits the space, then picks the preset it is already on.
    /// The host parameter does not move, so nothing but the link can put the
    /// chain back; the audio thread must end on the preset's values.
    #[test]
    fn picking_the_current_preset_again_puts_its_chain_back() {
        let preset = recipe(CHAMBER);
        let link = FxLink::new(preset.clone());
        let mut chain = Chain::new(SR, BLOCK);
        apply(&mut chain, &preset);
        let mut audio_seen = link.rev();

        let editor_seen = link.publish(pushed(preset.clone()));
        assert!(link.apply_if_new(&mut audio_seen, |s| { apply(&mut chain, s); }));
        assert_eq!(size(&chain), 0.9);
        assert_eq!(audio_seen, editor_seen);

        let _ = link.publish(preset.clone());
        assert!(link.apply_if_new(&mut audio_seen, |s| { apply(&mut chain, s); }));
        assert_eq!(size(&chain), CHAMBER.size);
        assert_eq!(chain.len(), FX_SLOTS);
    }

    /// A counter that did not move applies nothing.
    #[test]
    fn a_block_with_nothing_new_applies_nothing() {
        let link = FxLink::new(recipe(CHAMBER));
        let mut seen = link.rev();
        let mut ran = false;
        assert!(!link.apply_if_new(&mut seen, |_| ran = true));
        assert!(!ran);
        link.seed(ChainSpec::default());
        assert!(!link.apply_if_new(&mut seen, |_| ran = true), "a seed moves no counter");
    }
}
