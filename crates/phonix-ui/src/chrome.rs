//! The plugin window's header, and the side channel a standalone host uses
//! to show its device and take a restart request from it.

use egui::{Align, Color32, Layout, RichText, Ui, Vec2};

use crate::preset_picker;
use crate::theme::Palette;
use crate::widgets::peak_meter;

/// What the GUI is asking the host to do.
///
/// The GUI cannot perform either itself: the stream is not `Send` and the
/// callback factory lives in the binary that knows how to rebuild it. So the
/// editor raises a request and whoever owns the host drains it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioRequest {
    /// Close and reopen the audio device, picking up hardware changes.
    RestartAudio,
    /// Drop and rebuild the MIDI input connections.
    RescanMidi,
}

/// What the host wants to say back, for display.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct HostStatus {
    /// Device the stream is currently on, empty when stopped.
    pub device: String,
    pub backend: String,
    /// Number of connected MIDI inputs.
    pub midi_inputs: usize,
    /// Set when the last restart failed, so the GUI can say WHY the device did
    /// not come back rather than looking merely idle.
    pub error: Option<String>,
}

/// The status is published on the context before the app draws and the
/// request drained after it, so no constructor threads a host handle
/// through an editor that must not show the control in a DAW.
fn status_id() -> egui::Id { egui::Id::new("phonix_host_status") }
fn request_id() -> egui::Id { egui::Id::new("phonix_host_request") }

pub fn publish_status(ctx: &egui::Context, s: &HostStatus) {
    ctx.data_mut(|d| d.insert_temp(status_id(), s.clone()));
}

/// Read the status. `None` when nothing published one, which is exactly the
/// in-DAW case: there the host owns the device and the control must not
/// appear at all.
pub fn status(ctx: &egui::Context) -> Option<HostStatus> {
    ctx.data(|d| d.get_temp::<HostStatus>(status_id()))
}

pub fn request(ctx: &egui::Context, r: AudioRequest) {
    ctx.data_mut(|d| d.insert_temp(request_id(), r));
}

pub fn take_request(ctx: &egui::Context) -> Option<AudioRequest> {
    ctx.data_mut(|d| {
        let r = d.get_temp::<AudioRequest>(request_id());
        d.remove::<AudioRequest>(request_id());
        r
    })
}

// Shared header for every plugin window. Standardises the layout
// (title -> mode pills -> preset picker -> status -> meter) so each
// plugin no longer hand-rolls its own. The sync_cooldown / patch
// snapshot bookkeeping that every plugin keeps stays in the plugin —
// the chrome only renders the visuals + threads back which control
// the user touched.
//
// Designed so a plugin's draw_ui ends up roughly:
//
//     let res = chrome::plugin_chrome(ui, &chrome::PluginChrome {
//         title: "BASS", accent: ACCENT, dim: DIM,
//         peak: self.peak, preset_salt: "bass_preset",
//         mode_pills: &pills, status_right: None,
//     }, &mut self.preset_state, &presets);
//     if let Some(i) = res.pill_clicked   { ... }
//     if let Some(i) = res.preset_selected { ... }


/// One selectable pill in the header's mode strip.
pub struct PillEntry<'a> {
    pub label:    &'a str,
    pub selected: bool,
}

/// Inputs to `plugin_chrome`. Builder-style — set only what the
/// plugin actually has. Empty `mode_pills` skips the strip, `None`
/// status_right skips the status block.
pub struct PluginChrome<'a> {
    pub title:        &'a str,
    pub accent:       Color32,
    pub dim:          Color32,
    pub peak:         f32,
    /// CPU load 0..1, shown as a FIXED-WIDTH `CPU nnn%` readout in the right tail
    /// (uniform place + constant width across every plugin, so it never shifts the
    /// header). `None` = not shown.
    pub cpu:          Option<f32>,
    pub preset_salt:  &'a str,
    pub mode_pills:   &'a [PillEntry<'a>],
    pub status_right: Option<&'a str>,
}

/// What the chrome reports back to the plugin. The plugin reacts by
/// sending an engine command + updating its mirror.
pub struct ChromeResult {
    pub pill_clicked:    Option<usize>,
    pub preset_selected: Option<usize>,
    /// The "Save" disk button was clicked — the plugin should write its patch
    /// to disk (e.g. via `crate::preset_io::save_patch_to_disk`).
    pub save_clicked:    bool,
    /// The "Load" disk button was clicked — the plugin should load a patch from
    /// disk (e.g. via `crate::preset_io::load_patch_from_disk`).
    pub load_clicked:    bool,
}

/// Render the standard plugin header. Returns indices of any UI
/// events the plugin should react to.
///
/// Alignment discipline (the pre-fix chrome was visibly mismatched):
/// - Title uses `ui.label(...).size(CHROME_TITLE_PT)` not `ui.heading()`
///   (heading's default 17 pt towered over the 11 pt pills, creating
///   baseline drift and a tall row in pill-less plugins).
/// - Every text element on the row uses the same `Align::Center`
///   vertical alignment so pills, picker chrome and status text sit
///   on one horizontal line regardless of font size.
/// - The row height is pinned to `CHROME_ROW_H` so a pill-less
///   plugin and a pill plugin get identical
///   header heights and the outer frame doesn't jump.
/// - 12 px gap between every logical block (title | pills | picker)
///   plus a guaranteed 12 px gap between picker and the right-aligned
///   tail.
pub fn plugin_chrome<P: preset_picker::Preset>(
    ui:           &mut Ui,
    chrome:       &PluginChrome,
    preset_state: &mut preset_picker::PresetPickerState,
    presets:      &[P],
) -> ChromeResult {
    let mut pill_clicked = None;
    let mut preset_selected = None;
    let mut save_clicked = false;
    let mut load_clicked = false;

    ui.horizontal(|ui| {
        ui.set_min_height(CHROME_ROW_H);
        // Title — uses a fixed-size label rather than ui.heading() so
        // the row keeps a predictable baseline / height.
        ui.label(RichText::new(chrome.title)
            .color(chrome.accent).strong().size(CHROME_TITLE_PT));
        ui.add_space(CHROME_GAP);

        // Optional mode pills.
        for (i, pill) in chrome.mode_pills.iter().enumerate() {
            let col = if pill.selected { chrome.accent } else { chrome.dim };
            let text = RichText::new(pill.label).color(col).strong().size(CHROME_PILL_PT);
            if ui.selectable_label(pill.selected, text).clicked() {
                pill_clicked = Some(i);
            }
        }
        if !chrome.mode_pills.is_empty() { ui.add_space(CHROME_GAP); }

        // Preset picker — fixed 200 px wide widget that handles its
        // own internal layout. Identity-only style.
        let style = preset_picker::PresetPickerStyle {
            salt: chrome.preset_salt, accent: chrome.accent, dim: chrome.dim,
        };
        preset_selected = preset_picker::picker_ui(ui, &style, preset_state, presets);

        // Disk save/load, next to the factory-preset picker.
        ui.add_space(CHROME_GAP);
        if ui.small_button(RichText::new("Save").color(chrome.dim).size(CHROME_PILL_PT)).clicked() {
            save_clicked = true;
        }
        if ui.small_button(RichText::new("Load").color(chrome.dim).size(CHROME_PILL_PT)).clicked() {
            load_clicked = true;
        }

        // Right-aligned tail: status text (if any) then meter.
        // Guaranteed gap from the picker is enforced by the RTL block
        // starting only after `add_space` — picker ↔ tail can't collide.
        ui.add_space(CHROME_GAP);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            peak_meter(ui, chrome.peak, Vec2::new(60.0, 8.0), chrome.accent);
            // Fixed-width CPU readout, uniform across every plugin. `{:>3.0}` +
            // monospace keeps `CPU   5%` and `CPU 100%` the SAME width, so the
            // value never shifts the header.
            if let Some(cpu) = chrome.cpu {
                ui.add_space(8.0);
                let pct = (cpu * 100.0).clamp(0.0, 999.0);
                let col = if cpu > 0.85 { Color32::from_rgb(255, 70, 50) }
                    else if cpu > 0.6 { Color32::from_rgb(230, 180, 60) }
                    else { Color32::from_rgb(120, 190, 120) };
                ui.label(RichText::new(format!("CPU {pct:>3.0}%"))
                    .monospace().size(CHROME_STATUS_PT).color(col));
            }
            if let Some(s) = chrome.status_right {
                ui.add_space(8.0);
                ui.label(RichText::new(s).color(chrome.dim).size(CHROME_STATUS_PT));
            }
            // Device control, only where a host published its status. In the
            // DAW nothing does, because there the host owns the device and
            // reopening it from a plugin window would be wrong.
            if let Some(st) = status(ui.ctx()) {
                ui.add_space(8.0);
                let (txt, col) = match (&st.error, st.device.is_empty()) {
                    (Some(_), _)  => ("AUDIO !".to_string(), Color32::from_rgb(255, 110, 110)),
                    (None, true)  => ("AUDIO —".to_string(), Color32::from_rgb(255, 160, 90)),
                    (None, false) => (format!("{} · {} MIDI", st.device, st.midi_inputs), chrome.dim),
                };
                let resp = ui.add(egui::Label::new(
                    RichText::new(txt).color(col).size(CHROME_STATUS_PT))
                    .sense(egui::Sense::click()));
                let resp = if let Some(e) = &st.error {
                    resp.on_hover_text(e.as_str())
                } else {
                    resp.on_hover_text("click: reinitialise audio · right-click: rescan MIDI")
                };
                if resp.clicked() {
                    request(
                        ui.ctx(), AudioRequest::RestartAudio);
                }
                if resp.secondary_clicked() {
                    request(
                        ui.ctx(), AudioRequest::RescanMidi);
                }
            }
        });
    });

    ChromeResult { pill_clicked, preset_selected, save_clicked, load_clicked }
}
/// Pinned chrome row height (px). All title / pills / picker / status
/// share this row so the chrome looks identical across plugins.
const CHROME_ROW_H:    f32 = 26.0;
const CHROME_TITLE_PT: f32 = 14.0;
const CHROME_PILL_PT:  f32 = 11.0;
const CHROME_STATUS_PT:f32 = 10.0;
const CHROME_GAP:      f32 = 12.0;

/// The chrome in a `Panel::top` with one frame, so every plugin window gets
/// the same fill, border, margin and height whatever theme its body uses.
/// Call it from the root `Ui` before the body's central panel.
pub fn plugin_chrome_panel<P: preset_picker::Preset>(
    ui:           &mut Ui,
    chrome:       &PluginChrome,
    preset_state: &mut preset_picker::PresetPickerState,
    presets:      &[P],
) -> ChromeResult {
    let pal = Palette::of(ui.ctx());
    egui::Panel::top("plugin_chrome_panel")
        .frame(
            egui::Frame::NONE
                .fill(pal.bg_raised)
                .inner_margin(egui::Margin::symmetric(10i8, 5i8))
                .stroke(egui::Stroke::new(1.0_f32, pal.border)),
        )
        .show_inside(ui, |ui| plugin_chrome(ui, chrome, preset_state, presets))
        .inner
}

#[cfg(test)]
mod chrome_tests {
    use super::*;
    use crate::preset_picker::{NamedPreset, PresetPickerState};

    /// Smoke test: chrome with mode pills + status renders without
    /// panicking inside a synthetic egui context.
    #[test]
    fn chrome_renders_without_panic() {
        let ctx = egui::Context::default();
        let mut state = PresetPickerState::default();
        let presets = [
            NamedPreset { name: "A", category: Some("cat") },
            NamedPreset { name: "B", category: Some("cat") },
        ];
        let pills = [
            PillEntry { label: "M1", selected: true  },
            PillEntry { label: "M2", selected: false },
        ];
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let res = plugin_chrome(
                    ui,
                    &PluginChrome {
                        title: "TEST", accent: Color32::RED, dim: Color32::GRAY,
                        peak: 0.5, cpu: Some(0.42), preset_salt: "test",
                        mode_pills: &pills,
                        status_right: Some("8 voices"),
                    },
                    &mut state,
                    &presets,
                );
                // Defaults: nothing clicked the first frame.
                assert!(res.pill_clicked.is_none());
                assert!(res.preset_selected.is_none());
            });
        });
    }
}
