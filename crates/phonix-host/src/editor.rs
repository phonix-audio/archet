//! A plugin's OWN editor, in a window of ours.
//!
//! VST3 hands the host an `IPlugView` and an X11 window id to put it in; from
//! there the plugin draws itself. What is here is only the host half: open a
//! top-level window of the size the view asks for, hand over its id, and take
//! it away again cleanly when the window is closed or the plugin goes.
//!
//! Linux only, and X11 only. VST3 defines no platform type for Wayland, so
//! there is nothing to attach to there; callers fall back to the generic
//! parameter grid, which is also the fallback for a plugin with no editor.
//!
//! Deliberately absent: `IPlugFrame`. Setting one makes the plugin ask the
//! host for its run loop, and nice-plug aborts the process outright if
//! registering an event handler fails. Marteau's editor is a fixed size and
//! never asks to resize, and the plugins that do simply do not get to.

#![cfg(target_os = "linux")]

use std::ffi::c_void;

use vst3_sys::gui::IPlugView;
use vst3_sys::vst::IEditController;
use vst3_sys::VstPtr;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, WindowClass,
};
use x11rb::protocol::Event as X11Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

use crate::vst3::Vst3EditorConn;

/// The platform type an X11 host window is passed under. Spelled out because
/// `vst3-sys` does not carry the constant, and it has to match the plugin's
/// spelling exactly.
const PLATFORM_X11: &[u8] = b"X11EmbedWindowID\0";
/// The only view a VST3 plugin is required to offer.
const VIEW_EDITOR: &[u8] = b"editor\0";

/// What happened to the window since the last frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditorEvent {
    Nothing,
    /// The user closed it.
    Closed,
}

/// One plugin editor, in its own top-level X11 window.
pub struct Vst3EditorWindow {
    conn: RustConnection,
    window: u32,
    wm_delete: u32,
    view: VstPtr<dyn IPlugView>,
    /// The controller the view came from. Dropped after the view, so the
    /// plugin's code is still there while the view is being taken apart.
    _editor: Vst3EditorConn,
}

impl Vst3EditorWindow {
    /// Open the plugin's editor. `Err` means the caller should fall back to the
    /// generic grid: no editor, no X11, or a plugin that cannot be embedded the
    /// only way this platform offers.
    ///
    /// `app_id` is the host application's own window id, which the panel
    /// wears so a taskbar files it with whatever opened it.
    pub fn open(editor: Vst3EditorConn, title: &str, app_id: &str) -> Result<Self, String> {
        let raw = unsafe { editor.controller().create_view(VIEW_EDITOR.as_ptr() as *const i8) };
        if raw.is_null() {
            return Err("the plugin has no editor".into());
        }
        // create_view hands over a reference; `owned` takes it, and dropping
        // the VstPtr releases it.
        let view: VstPtr<dyn IPlugView> = unsafe { VstPtr::owned(raw as *mut _) }
            .ok_or_else(|| "the plugin returned a null view".to_string())?;

        let supported =
            unsafe { view.is_platform_type_supported(PLATFORM_X11.as_ptr() as *const i8) };
        if supported != vst3_sys::base::kResultOk {
            return Err("the editor cannot be embedded in an X11 window".into());
        }

        let mut rect = vst3_sys::gui::ViewRect { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { view.get_size(&mut rect) };
        let w = (rect.right - rect.left).max(64) as u16;
        let h = (rect.bottom - rect.top).max(64) as u16;

        let (conn, window, wm_delete) = make_window(w, h, title, app_id)?;

        let attached = unsafe {
            view.attached(window as usize as *mut c_void, PLATFORM_X11.as_ptr() as *const i8)
        };
        if attached != vst3_sys::base::kResultOk {
            let _ = conn.destroy_window(window);
            let _ = conn.flush();
            return Err(format!("the plugin refused to attach: {attached:#x}"));
        }

        Ok(Self { conn, window, wm_delete, view, _editor: editor })
    }

    /// Drain the window's events. Called once per host frame; the plugin drives
    /// its own drawing on its own thread, so there is nothing else to pump.
    pub fn poll(&mut self) -> EditorEvent {
        let mut out = EditorEvent::Nothing;
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if let X11Event::ClientMessage(msg) = event {
                if msg.data.as_data32()[0] == self.wm_delete {
                    out = EditorEvent::Closed;
                }
            }
        }
        out
    }
}

impl Drop for Vst3EditorWindow {
    fn drop(&mut self) {
        // Order matters: the plugin lets go of the window first, then the
        // window goes. The other way round leaves it drawing into an id the
        // server has already recycled.
        unsafe { self.view.removed() };
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

/// Pin a window to one size by making its minimum and maximum equal.
/// A top-level X11 window of a fixed size, named, that tells us when the
/// user closes it. Returns the connection, the window and the delete atom.
fn make_window(w: u16, h: u16, title: &str, app_id: &str) -> Result<(RustConnection, u32, u32), String> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| format!("no X11 display: {e}"))?;
    let screen = &conn.setup().roots[screen_num];
    let window = conn.generate_id().map_err(|e| e.to_string())?;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        screen.root,
        0,
        0,
        w,
        h,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(screen.black_pixel)
            .event_mask(EventMask::STRUCTURE_NOTIFY),
    )
    .map_err(|e| e.to_string())?;

    conn.change_property8(
        PropMode::REPLACE,
        window,
        u32::from(AtomEnum::WM_NAME),
        u32::from(AtomEnum::STRING),
        title.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    // The host's own id, in both halves of WM_CLASS: a panel that named the
    // toolkit instead would be filed by the desktop under whichever
    // application of the family installed an entry by that name, and matched
    // by no `StartupWMClass` at all.
    let mut class = Vec::with_capacity(app_id.len() * 2 + 2);
    class.extend_from_slice(app_id.as_bytes());
    class.push(0);
    class.extend_from_slice(app_id.as_bytes());
    class.push(0);
    conn.change_property8(
        PropMode::REPLACE,
        window,
        u32::from(AtomEnum::WM_CLASS),
        u32::from(AtomEnum::STRING),
        &class,
    )
    .map_err(|e| e.to_string())?;

    // The window manager's close button has to come back to us as an event
    // rather than killing the connection: the view must be detached before
    // the window goes away.
    let wm_protocols = conn
        .intern_atom(false, b"WM_PROTOCOLS")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    let wm_delete = conn
        .intern_atom(false, b"WM_DELETE_WINDOW")
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        wm_protocols,
        u32::from(AtomEnum::ATOM),
        &[wm_delete],
    )
    .map_err(|e| e.to_string())?;

    // Fixed size: VST3 views that cannot resize must not be resized, and a
    // window manager will happily do it unless told the limits are equal.
    set_fixed_size(&conn, window, w, h)?;

    conn.map_window(window).map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;
    Ok((conn, window, wm_delete))
}

/// A CLAP plugin's editor in a window of ours: the plugin draws into the
/// X11 window it is given as a parent.
pub struct ClapEditorWindow {
    conn: RustConnection,
    window: u32,
    wm_delete: u32,
    editor: crate::clap::ClapEditorConn,
}

impl ClapEditorWindow {
    pub fn open(editor: crate::clap::ClapEditorConn, title: &str, app_id: &str) -> Result<Self, String> {
        use clap_sys::ext::gui::{clap_window, clap_window_handle, CLAP_WINDOW_API_X11};
        if !editor.is_alive() {
            return Err("the plugin instance is gone".into());
        }
        let (plugin, gui) = (editor.plugin, editor.gui);
        unsafe {
            let ok = (*gui).is_api_supported.map(|f| f(plugin, CLAP_WINDOW_API_X11.as_ptr(), false)).unwrap_or(false);
            if !ok { return Err("the editor cannot be embedded in an X11 window".into()); }
            if !(*gui).create.map(|f| f(plugin, CLAP_WINDOW_API_X11.as_ptr(), false)).unwrap_or(false) {
                return Err("the plugin refused to create its editor".into());
            }
            let (mut w, mut h) = (0u32, 0u32);
            let _ = (*gui).get_size.map(|f| f(plugin, &mut w, &mut h));
            let (conn, window, wm_delete) = match make_window(w.clamp(64, 4096) as u16, h.clamp(64, 4096) as u16, title, app_id) {
                Ok(v) => v,
                Err(e) => { if let Some(d) = (*gui).destroy { d(plugin); } return Err(e); }
            };
            let parent = clap_window { api: CLAP_WINDOW_API_X11.as_ptr(), specific: clap_window_handle { x11: window as std::ffi::c_ulong } };
            if !(*gui).set_parent.map(|f| f(plugin, &parent)).unwrap_or(false) {
                if let Some(d) = (*gui).destroy { d(plugin); }
                let _ = conn.destroy_window(window);
                let _ = conn.flush();
                return Err("the plugin refused the window".into());
            }
            let _ = (*gui).show.map(|f| f(plugin));
            Ok(Self { conn, window, wm_delete, editor })
        }
    }

    pub fn poll(&mut self) -> EditorEvent {
        let mut out = EditorEvent::Nothing;
        while let Ok(Some(event)) = self.conn.poll_for_event() {
            if let X11Event::ClientMessage(msg) = event {
                if msg.data.as_data32()[0] == self.wm_delete {
                    out = EditorEvent::Closed;
                }
            }
        }
        out
    }
}

impl Drop for ClapEditorWindow {
    fn drop(&mut self) {
        // Only while the instance is still there. A host that rebuilds its
        // engines destroys the plugin under an open editor, and these are raw
        // pointers into it; the plugin's own `destroy` takes its editor down
        // with it, so there is nothing left to close here.
        if self.editor.is_alive() {
            unsafe {
                let gui = self.editor.gui;
                let _ = (*gui).hide.map(|f| f(self.editor.plugin));
                if let Some(d) = (*gui).destroy { d(self.editor.plugin); }
            }
        }
        let _ = self.conn.destroy_window(self.window);
        let _ = self.conn.flush();
    }
}

/// Either format's editor window.
pub enum NativeEditorWindow {
    Vst3(Vst3EditorWindow),
    Clap(ClapEditorWindow),
}

impl NativeEditorWindow {
    /// `app_id` is the host application's own window id; the panel wears it
    /// so a taskbar files the two windows together.
    pub fn open(editor: crate::plugin::HostedEditor, title: &str, app_id: &str) -> Result<Self, String> {
        match editor {
            crate::plugin::HostedEditor::Vst3(c) => Vst3EditorWindow::open(c, title, app_id).map(NativeEditorWindow::Vst3),
            crate::plugin::HostedEditor::Clap(c) => ClapEditorWindow::open(c, title, app_id).map(NativeEditorWindow::Clap),
        }
    }

    pub fn poll(&mut self) -> EditorEvent {
        match self { NativeEditorWindow::Vst3(w) => w.poll(), NativeEditorWindow::Clap(w) => w.poll() }
    }
}

fn set_fixed_size(conn: &RustConnection, window: u32, w: u16, h: u16) -> Result<(), String> {
    // WM_NORMAL_HINTS, laid out as the ICCCM says: flags, then four padding
    // fields, then min and max size. PMinSize | PMaxSize = 0x30.
    let mut hints = [0u32; 18];
    hints[0] = 0x30;
    hints[5] = w as u32;
    hints[6] = h as u32;
    hints[7] = w as u32;
    hints[8] = h as u32;
    conn.change_property32(
        PropMode::REPLACE,
        window,
        u32::from(AtomEnum::WM_NORMAL_HINTS),
        u32::from(AtomEnum::WM_SIZE_HINTS),
        &hints,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Whether an X11 display can be reached at all.
///
/// The one check that decides between a native editor and the generic grid:
/// under Wayland, or with no display, there is no VST3 platform type to embed
/// into and the grid is the only interface a plugin has.
pub fn x11_available() -> bool {
    x11rb::connect(None).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a host would pass for its own windows.
    const APP_ID: &str = "phonix-host-test";

    /// Open a real plugin's real editor and take it down again.
    ///
    /// Ignored: it needs the bundle installed and a display. It is still the
    /// only thing that proves the sequence — create the view, claim the X11
    /// platform type, attach a window id — against a plugin rather than
    /// against the documentation.
    #[test]
    #[ignore = "needs the Phonix Piano bundle installed and an X11 display"]
    fn the_plugin_editor_attaches_to_a_window_of_ours() {
        if !x11_available() {
            eprintln!("no X11 display; skipping");
            return;
        }
        let Some(path) = crate::vst3::find_by_name("Phonix Piano") else {
            eprintln!("Phonix Piano is not installed; skipping");
            return;
        };
        let plugin = crate::vst3::Vst3Plugin::load(&path, 48_000.0, 512).expect("load");
        let conn = plugin.editor_conn().expect("the plugin publishes no controller");

        let win = Vst3EditorWindow::open(conn, "Phonix Piano (test)", APP_ID)
            .expect("the editor refused to attach");
        // Dropping detaches the view and destroys the window; a crash here is
        // the failure this test exists to catch.
        drop(win);
    }

    /// The same window, held open long enough to be looked at (or
    /// photographed). Separate from the attach test so that one stays fast.
    #[test]
    #[ignore = "needs an X11 display and the FX rack built"]
    fn a_clap_editor_attaches_to_a_window_of_ours() {
        if !x11_available() {
            eprintln!("no X11 display; skipping");
            return;
        }
        let so = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/libphonix_fx_rack.so");
        if !so.exists() {
            eprintln!("the FX rack is not built; skipping");
            return;
        }
        let plugin = crate::clap::ClapPlugin::load(&so, None, 48_000.0, 512).expect("load");
        let conn = plugin.editor_conn().expect("the plugin publishes no editor");
        let win = ClapEditorWindow::open(conn, "FX rack (test)", APP_ID).expect("the editor refused to attach");
        std::thread::sleep(std::time::Duration::from_millis(300));
        drop(win);
    }

    /// The crash this guards: a host that adds a track while a CLAP editor
    /// is open rebuilds its engines, which destroys the plugin instance the
    /// open window holds raw pointers into. Closing that window afterwards
    /// called `hide` and `destroy` through freed memory, and the process
    /// died inside the plugin.
    #[test]
    fn a_window_outliving_its_plugin_calls_nothing() {
        let Some(path) = clap_under_test() else {
            eprintln!("no .clap to test with; skipping");
            return;
        };
        let plugin = crate::clap::ClapPlugin::load(&path, None, 48_000.0, 512).expect("load");
        let conn = plugin.editor_conn().expect("the plugin publishes no editor");
        assert!(conn.is_alive());
        drop(plugin);
        assert!(!conn.is_alive(), "the connection still claims a destroyed plugin");
        // What the host does next frame: it tries to reopen the editor.
        assert!(ClapEditorWindow::open(conn, "gone", APP_ID).is_err());
    }

    /// The same, with a window actually open across the plugin's destruction:
    /// exactly the user's sequence.
    #[test]
    #[ignore = "needs an X11 display"]
    fn an_open_window_survives_its_plugin_being_destroyed() {
        if !x11_available() {
            eprintln!("no X11 display; skipping");
            return;
        }
        let Some(path) = clap_under_test() else {
            eprintln!("no .clap to test with; skipping");
            return;
        };
        let plugin = crate::clap::ClapPlugin::load(&path, None, 48_000.0, 512).expect("load");
        let conn = plugin.editor_conn().expect("the plugin publishes no editor");
        let mut win = ClapEditorWindow::open(conn, "closing under it", APP_ID).expect("attach");
        drop(plugin);
        let _ = win.poll();
        drop(win);
    }

    /// A `.clap` to test against: one this repository builds, or one
    /// installed for the user.
    fn clap_under_test() -> Option<std::path::PathBuf> {
        let so = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/libphonix_fx_rack.so");
        if so.exists() { return Some(so); }
        let home = std::env::var_os("HOME")?;
        let dir = std::path::Path::new(&home).join(".clap");
        std::fs::read_dir(dir).ok()?.flatten()
            .map(|e| e.path())
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some("clap"))
    }

    #[test]
    #[ignore = "opens a window for several seconds"]
    fn the_editor_can_be_seen() {
        if !x11_available() {
            eprintln!("no X11 display; skipping");
            return;
        }
        let Some(path) = crate::vst3::find_by_name("Phonix Piano") else {
            eprintln!("Phonix Piano is not installed; skipping");
            return;
        };
        let plugin = crate::vst3::Vst3Plugin::load(&path, 48_000.0, 512).expect("load");
        let conn = plugin.editor_conn().expect("no controller");
        let mut win = Vst3EditorWindow::open(conn, "Phonix Piano", APP_ID).expect("attach");
        for _ in 0..60 {
            if win.poll() == EditorEvent::Closed {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
