//! The editor layer: the design system as a value (`theme`), the widgets,
//! icons, the piano keyboard, the preset picker and its on-disk dialogs,
//! the plugin window's chrome, and, behind `ui-spec`, the declarative
//! layout renderer. Nothing here knows an engine, a patch type or a host:
//! widgets take values and return events.

pub use egui;

pub mod chrome;
pub mod colors;
pub mod icons;
pub mod keyboard;
pub mod preset_io;
pub mod preset_picker;
#[cfg(feature = "rack")]
pub mod rack;
pub mod theme;
#[cfg(feature = "ui-spec")]
pub mod ui_spec;
pub mod widgets;

pub use theme::Palette;
