# Phonix SDK

The crates every Phonix instrument and the sequencer build from, as one
workspace with one version and one tag.

| crate | depends on | what it is |
|---|---|---|
| `phonix-rt` | nothing | the audio thread's toolbox: triple buffer, denormal guard, delay compensation |
| `phonix-dsp` | serde, rustfft | audio primitives: filters, oscillators, LFOs, envelopes, granular, spectral, pitch, reverbs, choruses, meters |
| `phonix-music` | serde | notes, time and theory: `MidiNote`, `PPQN`, scales, arpeggiator, groove |
| `phonix-fx` | serde | the effects contract: `Effect`, `EffectSpec`, `Registry`, `Chain`, `ChainSpec`; every effect behind a feature |
| `phonix-plugin` | serde | `Preset`, the `.vstpreset` writer, `Identity`, `Instrument` |
| `phonix-ui` | egui, phonix-plugin | the design system as a value, widgets, keyboard, preset picker, plugin chrome; `ui-spec` feature for the layout renderer |
| `phonix-session` | phonix-music, phonix-fx, phonix-legacy | the sequencer's session |
| `phonix-legacy` | phonix-fx | reads effect state written before names were the wire format |

Depend on a tag, never on a branch:

    phonix-fx = { git = "https://github.com/phonix-audio/phonix-sdk.git", tag = "v0.2.0" }

A crate boundary here keeps a dependency out: egui stops at `phonix-ui`, the
plugin framework never enters, the session model reaches no plugin. The
compatibility surface is the string ids of effect kinds and parameters, not
the Rust API.

CI builds the workspace from a clone with no sibling checkout, on Linux and
Windows, and fails if any crate below `phonix-ui` pulls egui in.

## Licence

MIT. See `LICENSE-MIT`.
