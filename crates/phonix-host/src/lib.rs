//! Hosting a plugin.
//!
//! What it takes to play an instrument that lives in a bundle: load it,
//! whichever of the two formats it came in, open an audio device, and run one
//! against the other. A sequencer does this once per track; a standalone host
//! does it once. Both want the same code, and before this crate existed there
//! were two of it.
//!
//! The pieces:
//!
//!   `vst3`, `clap`  one plugin format each, down to the raw ABI
//!   `plugin`        one surface over the two, and the playhead they read
//!   `audio`         the sound card, through JACK where there is one
//!   `audio_host`    an audio device and a MIDI input, started and restarted
//!   `editor`        the plugin's own window, on the platforms that have one
//!   `rt`            what a process does once so its audio thread can run
//!
//! Nothing here draws anything, and nothing here reads a configuration file:
//! where a host keeps its settings, and what it looks like, is the host's.

pub mod audio;
pub mod audio_host;
pub mod clap;
#[cfg(target_os = "linux")]
pub mod editor;
pub mod plugin;
pub mod rt;
pub mod vst3;

pub use plugin::{HostedEditor, HostedPlugin, PlayHead};
pub use vst3::{ParamEdit, Vst3Plugin, Vst3PluginInfo};
pub use clap::{ClapPlugin, ClapPluginInfo};
#[cfg(target_os = "linux")]
pub use editor::{EditorEvent, NativeEditorWindow};
