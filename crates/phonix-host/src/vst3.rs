//! VST3 instrument plugin hosting for PhonixSeq.
//!
//! Loads .vst3 plugins from disk and processes MIDI→audio on the audio thread.
//! Scope: instrument plugins (MIDI → audio, `process`) and insert effects
//! (audio → audio, `process_effect`). No native editor embedding.

#![allow(non_snake_case)]
#![allow(clippy::missing_safety_doc)]

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};

// COM initialization — required on any thread that calls into VST3 plugins.
#[cfg(target_os = "windows")]
#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> i32;
}
#[cfg(target_os = "windows")]
const COINIT_MULTITHREADED: u32 = 0x0;

use libloading::{Library, Symbol};

use vst3_sys::base::{
    char8, IBStream, IPluginBase, IPluginFactory, IPluginFactory2, IPluginFactory3, PClassInfo, PClassInfo2,
    kIBSeekCur, kIBSeekEnd, kIBSeekSet, kResultFalse, kResultOk, kNotImplemented,
};
use vst3_sys::vst::{IComponentHandler, IHostApplication};
use vst3_sys::utils::{SharedVstPtr, StaticVstPtr};
use vst3_sys::vst::{
    AudioBusBuffers, BusDirections, Event, EventData, EventTypes, IAudioProcessor, IComponent,
    IEditController, IEventList, IMidiMapping, IParamValueQueue, IParameterChanges, NoteOffEvent,
    NoteOnEvent, ParameterFlags, ParameterInfo, ProcessContext, ProcessData, ProcessSetup,
    kStereo, MediaTypes,
};
use vst3_sys::{VST3, VstPtr};

type GetPluginFactoryFn = unsafe extern "system" fn() -> *mut c_void;

/// `ParameterInfo::kIsHidden`. `vst3-sys` stops its `ParameterFlags` enum at
/// `kIsBypass`, so the bit is spelled out here rather than guessed at each use.
const PARAM_IS_HIDDEN: i32 = 1 << 4;

/// The two controllers VST3 numbers past the MIDI ones. `IMidiMapping`
/// publishes a parameter for each like any other controller, and that
/// parameter is the only path either has into a VST3 plugin.
pub const AFTERTOUCH: u8 = 128;
pub const PITCH_BEND: u8 = 129;
/// How many controllers the plugin is asked about: 0..127 and those two.
const CONTROLLER_COUNT: u8 = PITCH_BEND + 1;

// ── Public info types ─────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Vst3PluginInfo {
    pub name:   String,
    pub vendor: String,
    pub path:   PathBuf,
    /// The first audio-effect class's sub-categories, as the factory
    /// declares them ("Instrument", "Synth", "Fx", "Reverb", ...).
    pub subcategories: Vec<String>,
    /// Declared under `Instrument` or `Fx`. A bundle that could not be
    /// opened declares neither and stays listed.
    pub is_instrument: bool,
    pub is_effect: bool,
}

impl Vst3PluginInfo {
    /// What a browser groups the plugin under: the first sub-category
    /// that is not the instrument/effect switch itself.
    pub fn category(&self) -> Option<&str> {
        self.subcategories.iter()
            .map(String::as_str)
            .find(|c| !matches!(*c, "Instrument" | "Fx"))
    }
}

/// One parameter, as the plugin describes it.
///
/// The host used to keep only id, title and value, and drew every parameter as
/// a bare 0..1 slider — a filter type became "0.67", a bypass became a slider.
/// Everything here is published by `IEditController` and was simply thrown away.
#[derive(Clone, Debug, Default)]
pub struct Vst3ParamEntry {
    pub id:    u32,
    pub title: String,
    /// Abbreviated title, for narrow layouts. Often empty.
    pub short_title: String,
    /// "dB", "Hz", "%"… as the plugin spells it. Often empty.
    pub units: String,
    /// 0 continuous, 1 a toggle, n a switch with n+1 positions.
    pub step_count: i32,
    pub value: f64,   // normalized 0..1
    pub default: f64, // normalized 0..1
    pub flags: i32,
    /// The plugin's OWN formatted value ("-6.0 dB", "Lowpass 24"). Worth far
    /// more than the raw normalized number, and only the plugin can produce it.
    pub display: String,
    /// For a `kIsList` parameter, one label per position.
    pub enum_labels: Vec<String>,
}

impl Vst3ParamEntry {
    pub fn is_read_only(&self) -> bool { self.flags & (ParameterFlags::kIsReadOnly as i32) != 0 }
    pub fn is_bypass(&self)    -> bool { self.flags & (ParameterFlags::kIsBypass as i32) != 0 }
    pub fn is_list(&self)      -> bool { self.flags & (ParameterFlags::kIsList as i32) != 0 }
    pub fn is_toggle(&self)    -> bool { self.step_count == 1 }
}

#[derive(Clone, Debug, Default)]
pub struct Vst3ParamCache {
    pub params:      Vec<Vst3ParamEntry>,
    pub plugin_name: String,
}

// ── HostApplication — minimal IHostApplication so plugins don't crash on init ─

#[VST3(implements(IHostApplication))]
pub struct HostApplication {}

impl HostApplication {
    pub fn new() -> Box<Self> {
        Self::allocate()
    }

    /// Return a raw `*mut c_void` COM pointer suitable for `IPluginFactory3::set_host_context`.
    /// The pointer is valid for as long as `boxed` is alive.
    pub fn as_context(boxed: &Box<Self>) -> *mut c_void {
        &**boxed as *const Self as *mut Self as *mut c_void
    }
}

unsafe impl Send for HostApplication {}

impl IHostApplication for HostApplication {
    unsafe fn get_name(&self, name: *mut u16) -> i32 {
        // Write "PhonixSeq\0" as UTF-16 into the 128-char buffer
        if name.is_null() { return kResultOk; }
        let label: &[u16] = &[b'V' as u16, b'o' as u16, b'x' as u16,
                               b'S' as u16, b'e' as u16, b'q' as u16, 0];
        for (i, &ch) in label.iter().enumerate() {
            *name.add(i) = ch;
        }
        kResultOk
    }

    unsafe fn create_instance(
        &self,
        _cid: *const vst3_com::IID,
        _iid: *const vst3_com::IID,
        obj: *mut *mut c_void,
    ) -> i32 {
        // Null the output so callers don't dereference garbage on failure
        if !obj.is_null() { *obj = std::ptr::null_mut(); }
        kNotImplemented
    }
}

/// `RestartFlags`, from the VST3 SDK.
///
/// A wire format: the plugin picks the bits, the host reads them. vst3-sys
/// does not declare them, so they are written out here against
/// `ivsteditcontroller.h` and must not be renumbered.
pub mod restart {
    pub const RELOAD_COMPONENT: i32 = 1 << 0;
    pub const IO_CHANGED: i32 = 1 << 1;
    pub const PARAM_VALUES_CHANGED: i32 = 1 << 2;
    pub const LATENCY_CHANGED: i32 = 1 << 3;
    pub const PARAM_TITLES_CHANGED: i32 = 1 << 4;
    pub const MIDI_CC_ASSIGNMENT_CHANGED: i32 = 1 << 5;
    pub const NOTE_EXPRESSION_CHANGED: i32 = 1 << 6;
    pub const IO_TITLES_CHANGED: i32 = 1 << 7;
    pub const PREFETCHABLE_SUPPORT_CHANGED: i32 = 1 << 8;
    pub const ROUTING_INFO_CHANGED: i32 = 1 << 9;

    /// The ones that make a cached view of the plugin's parameters stale.
    pub const WORTH_RELOADING: i32 =
        RELOAD_COMPONENT | PARAM_VALUES_CHANGED | PARAM_TITLES_CHANGED | LATENCY_CHANGED;
}

// ── HostComponentHandler — where a plugin's own editor reports its edits ─────

/// What a plugin's own window did, on its way back to the host.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamEdit {
    /// A parameter was moved. `value` is normalised, 0..1.
    Set { param_id: u32, value: f64 },
    /// The plugin says its parameters, their titles or its latency changed
    /// under the host -- a preset loaded in its own window, typically. A
    /// cached view of them is now stale.
    Reload,
}

/// The host side of a parameter edit made in the plugin's window.
///
/// Without one of these a plugin's editor is a picture: nice-plug writes the
/// plugin's own parameters through `IComponentHandler::perform_edit` and
/// silently does nothing when no handler is set, and the VST3 SDK says the
/// same of any plugin that automates.
///
/// Where the edit GOES is the host's business: a sequencer sends it down a
/// command channel to its audio thread, a standalone host may apply it on the
/// spot. So this takes a callback and knows nothing about either.
#[VST3(implements(IComponentHandler))]
pub struct HostComponentHandler {
    report: Box<dyn Fn(ParamEdit) + Send + Sync>,
}

impl HostComponentHandler {
    pub fn new(report: Box<dyn Fn(ParamEdit) + Send + Sync>) -> Box<Self> {
        Self::allocate(report)
    }

    /// A borrowed COM pointer for `set_component_handler`. Valid as long as the
    /// box lives, which is why the plugin keeps it.
    pub fn as_shared(boxed: &Box<Self>) -> SharedVstPtr<dyn IComponentHandler> {
        unsafe { std::mem::transmute(&**boxed as *const Self as *mut Self) }
    }

    /// The null handler, for handing the plugin back its `nullptr` before this
    /// box goes away. `SharedVstPtr` is a transparent raw pointer and the crate
    /// checks it for null (`is_null`, `upgrade`), so this is the value the API
    /// expects, not a trick.
    pub fn none() -> SharedVstPtr<dyn IComponentHandler> {
        unsafe { std::mem::transmute(std::ptr::null_mut::<Self>()) }
    }
}

unsafe impl Send for HostComponentHandler {}

impl IComponentHandler for HostComponentHandler {
    /// Automation gesture start. There is no gesture to open until a host
    /// records VST3 parameter automation.
    unsafe fn begin_edit(&self, _id: u32) -> i32 {
        kResultOk
    }

    unsafe fn perform_edit(&self, id: u32, value_normalized: f64) -> i32 {
        (self.report)(ParamEdit::Set { param_id: id, value: value_normalized });
        kResultOk
    }

    unsafe fn end_edit(&self, _id: u32) -> i32 {
        kResultOk
    }

    unsafe fn restart_component(&self, flags: i32) -> i32 {
        if flags & restart::WORTH_RELOADING != 0 {
            (self.report)(ParamEdit::Reload);
        }
        kResultOk
    }
}

// ── Vst3EditorConn — the controller, reachable from the GUI thread ───────────

/// One COM reference to a loaded plugin's edit controller, plus the module it
/// came from, so a window can be opened against it away from the audio thread.
///
/// Cloning it takes another COM reference. The GUI keeps its copy in the track
/// handle and clones per window, so closing an editor and opening it again
/// works — taking the reference out of the handle would leave the second
/// attempt with nothing and silently downgrade the track to the generic grid.
#[derive(Clone)]
pub struct Vst3EditorConn {
    ctrl:    VstPtr<dyn IEditController>,
    /// Keeps the shared library mapped. Dropping the last one unloads the code
    /// the view is running.
    _module: std::sync::Arc<Vst3Module>,
}

impl Vst3EditorConn {
    pub fn controller(&self) -> &VstPtr<dyn IEditController> {
        &self.ctrl
    }
}

// The controller is a single COM object shared with the audio thread, which
// only ever reads parameter values through it. Moving this handle to the GUI
// thread is the arrangement every VST3 host uses.
unsafe impl Send for Vst3EditorConn {}

// ── HostStream — implements IBStream for get_state / set_state ────────────────

#[VST3(implements(IBStream))]
pub struct HostStream {
    pub data: UnsafeCell<Vec<u8>>,
    pub pos:  UnsafeCell<usize>,
}

impl HostStream {
    pub fn write_stream() -> Box<Self> {
        Self::allocate(UnsafeCell::new(Vec::new()), UnsafeCell::new(0))
    }

    pub fn read_stream(data: Vec<u8>) -> Box<Self> {
        Self::allocate(UnsafeCell::new(data), UnsafeCell::new(0))
    }

    /// Collect data written via write().
    /// # Safety: must not be called while another reference holds the stream.
    pub fn into_bytes(boxed: Box<Self>) -> Vec<u8> {
        unsafe { (*boxed.data.get()).clone() }
    }

    /// Get a `SharedVstPtr` pointing to this stream for passing to VST3 methods.
    /// The returned pointer is valid for as long as `boxed` is alive.
    pub fn as_shared(boxed: &mut Box<Self>) -> SharedVstPtr<dyn IBStream> {
        unsafe { std::mem::transmute(&mut **boxed as *mut Self) }
    }
}

unsafe impl Send for HostStream {}

impl IBStream for HostStream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        num_bytes: i32,
        num_bytes_read: *mut i32,
    ) -> i32 {
        let data = &*self.data.get();
        let pos  = &mut *self.pos.get();
        let avail   = data.len().saturating_sub(*pos);
        let to_read = (num_bytes as usize).min(avail);
        if to_read > 0 {
            std::ptr::copy_nonoverlapping(
                data[*pos..].as_ptr(),
                buffer as *mut u8,
                to_read,
            );
            *pos += to_read;
        }
        if !num_bytes_read.is_null() { *num_bytes_read = to_read as i32; }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *const c_void,
        num_bytes: i32,
        num_bytes_written: *mut i32,
    ) -> i32 {
        let data  = &mut *self.data.get();
        let bytes = std::slice::from_raw_parts(buffer as *const u8, num_bytes as usize);
        data.extend_from_slice(bytes);
        if !num_bytes_written.is_null() { *num_bytes_written = num_bytes; }
        kResultOk
    }

    unsafe fn seek(&self, pos: i64, mode: i32, result: *mut i64) -> i32 {
        let data = &*self.data.get();
        let cur  = &mut *self.pos.get();
        let new_pos: usize = if mode == kIBSeekSet {
            pos as usize
        } else if mode == kIBSeekCur {
            (*cur as i64 + pos) as usize
        } else if mode == kIBSeekEnd {
            (data.len() as i64 + pos) as usize
        } else {
            return kResultFalse;
        };
        *cur = new_pos.min(data.len());
        if !result.is_null() { *result = *cur as i64; }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut i64) -> i32 {
        if !pos.is_null() { *pos = *self.pos.get() as i64; }
        kResultOk
    }
}

// ── HostEventList — implements IEventList for passing MIDI events to plugins ──

#[VST3(implements(IEventList))]
pub struct HostEventList {
    pub events: UnsafeCell<Vec<Event>>,
}

impl HostEventList {
    pub fn new() -> Box<Self> {
        Self::allocate(UnsafeCell::new(Vec::new()))
    }

    pub fn clear(boxed: &mut Box<Self>) {
        unsafe { (*boxed.events.get()).clear(); }
    }

    pub fn push_event(boxed: &mut Box<Self>, e: Event) {
        unsafe { (*boxed.events.get()).push(e); }
    }

    /// Get a `StaticVstPtr` for use in `ProcessData.input_events`.
    /// Valid for as long as `boxed` is alive.
    pub fn as_static(boxed: &mut Box<Self>) -> StaticVstPtr<dyn IEventList> {
        unsafe { std::mem::transmute(&mut **boxed as *mut Self) }
    }
}

unsafe impl Send for HostEventList {}

impl IEventList for HostEventList {
    unsafe fn get_event_count(&self) -> i32 {
        (*self.events.get()).len() as i32
    }

    unsafe fn get_event(&self, index: i32, e: *mut Event) -> i32 {
        let events = &*self.events.get();
        if index < 0 || index as usize >= events.len() {
            return kResultFalse;
        }
        *e = events[index as usize];
        kResultOk
    }

    unsafe fn add_event(&self, e: *mut Event) -> i32 {
        (*self.events.get()).push(*e);
        kResultOk
    }
}

// ── HostParamChanges / HostParamQueue — a real IParameterChanges ─────────────
//
// VST3 has no MIDI controller event. A controller arrives as a PARAMETER CHANGE
// whose id the plugin publishes through `IMidiMapping`. This used to be a stub
// reporting zero changes every block, which is why a hosted instrument could
// never be sent a sustain pedal — and a piano without a pedal is not a piano.
//
// Queues are reused between blocks, so the audio thread stops allocating once
// the set of touched parameters settles.

#[VST3(implements(IParamValueQueue))]
pub struct HostParamQueue {
    id: UnsafeCell<u32>,
    /// (sample offset, normalized value), kept ordered by offset as VST3 wants.
    points: UnsafeCell<Vec<(i32, f64)>>,
}

impl HostParamQueue {
    fn new() -> Box<Self> {
        Self::allocate(UnsafeCell::new(0), UnsafeCell::new(Vec::with_capacity(16)))
    }
}

unsafe impl Send for HostParamQueue {}

impl IParamValueQueue for HostParamQueue {
    unsafe fn get_parameter_id(&self) -> u32 { *self.id.get() }

    unsafe fn get_point_count(&self) -> i32 { (*self.points.get()).len() as i32 }

    unsafe fn get_point(&self, index: i32, sample_offset: *mut i32, value: *mut f64) -> i32 {
        if index < 0 { return kResultFalse; }
        let points = &*self.points.get();
        let Some(&(offset, v)) = points.get(index as usize) else { return kResultFalse };
        if !sample_offset.is_null() { *sample_offset = offset; }
        if !value.is_null() { *value = v; }
        kResultOk
    }

    unsafe fn add_point(&self, sample_offset: i32, value: f64, index: *mut i32) -> i32 {
        let points = &mut *self.points.get();
        let at = points.partition_point(|&(o, _)| o <= sample_offset);
        points.insert(at, (sample_offset, value));
        if !index.is_null() { *index = at as i32; }
        kResultOk
    }
}

#[VST3(implements(IParameterChanges))]
pub struct HostParamChanges {
    queues: UnsafeCell<Vec<Box<HostParamQueue>>>,
    /// How many of `queues` carry this block's changes. The rest are kept
    /// allocated for reuse.
    live: UnsafeCell<usize>,
}

impl HostParamChanges {
    pub fn new() -> Box<Self> {
        Self::allocate(UnsafeCell::new(Vec::new()), UnsafeCell::new(0))
    }

    /// Drop the previous block's points without giving back the memory.
    pub fn clear(boxed: &mut Box<Self>) {
        unsafe {
            for q in (*boxed.queues.get()).iter_mut() {
                (*q.points.get()).clear();
            }
            *boxed.live.get() = 0;
        }
    }

    /// Queue one change. `value` is normalized 0..1, `sample_offset` is where in
    /// the block it lands.
    pub fn push(boxed: &mut Box<Self>, id: u32, sample_offset: i32, value: f64) {
        unsafe {
            let queues = &mut *boxed.queues.get();
            let live = &mut *boxed.live.get();
            // One queue per parameter per block, as the interface requires.
            for q in queues[..*live].iter_mut() {
                if *q.id.get() == id {
                    q.add_point(sample_offset, value, std::ptr::null_mut());
                    return;
                }
            }
            if *live == queues.len() { queues.push(HostParamQueue::new()); }
            let q = &mut queues[*live];
            *q.id.get() = id;
            (*q.points.get()).clear();
            q.add_point(sample_offset, value, std::ptr::null_mut());
            *live += 1;
        }
    }

    pub fn as_static(boxed: &mut Box<Self>) -> StaticVstPtr<dyn IParameterChanges> {
        unsafe { std::mem::transmute(&mut **boxed as *mut Self) }
    }
}

unsafe impl Send for HostParamChanges {}

impl IParameterChanges for HostParamChanges {
    unsafe fn get_parameter_count(&self) -> i32 { *self.live.get() as i32 }

    unsafe fn get_parameter_data(&self, index: i32) -> StaticVstPtr<dyn IParamValueQueue> {
        if index < 0 || index as usize >= *self.live.get() {
            return std::mem::transmute(std::ptr::null_mut::<c_void>());
        }
        // Each queue is boxed, so its address survives the Vec growing.
        let queues = &mut *self.queues.get();
        std::mem::transmute(&mut *queues[index as usize] as *mut HostParamQueue)
    }

    unsafe fn add_parameter_data(
        &self,
        _id: *const u32,
        _index: *mut i32,
    ) -> StaticVstPtr<dyn IParamValueQueue> {
        // Only the host fills the input changes, and it uses `push` above.
        std::mem::transmute(std::ptr::null_mut::<c_void>())
    }
}

// ── Vst3Module ────────────────────────────────────────────────────────────────
//
// The VST3 layout requires a module entry point to run BEFORE `GetPluginFactory`
// and a matching exit after everything obtained from the factory is released.
// This host called neither: it went straight from `dlopen` to `GetPluginFactory`.
// Our own plugins only set up a logger there, which is why it went unnoticed on
// Windows, but a third-party plugin is entitled to do real work in it.

#[cfg(all(target_family = "unix", not(target_os = "macos")))]
type ModuleEntryFn = unsafe extern "C" fn(*mut c_void) -> bool;
#[cfg(all(target_family = "unix", not(target_os = "macos")))]
type ModuleExitFn = unsafe extern "C" fn() -> bool;
#[cfg(all(target_family = "unix", not(target_os = "macos")))]
const MODULE_ENTRY_SYM: &[u8] = b"ModuleEntry\0";
#[cfg(all(target_family = "unix", not(target_os = "macos")))]
const MODULE_EXIT_SYM: &[u8] = b"ModuleExit\0";

#[cfg(target_os = "windows")]
type ModuleEntryFn = unsafe extern "system" fn() -> bool;
#[cfg(target_os = "windows")]
type ModuleExitFn = unsafe extern "system" fn() -> bool;
#[cfg(target_os = "windows")]
const MODULE_ENTRY_SYM: &[u8] = b"InitDll\0";
#[cfg(target_os = "windows")]
const MODULE_EXIT_SYM: &[u8] = b"ExitDll\0";

#[cfg(target_os = "macos")]
type ModuleEntryFn = unsafe extern "C" fn(*mut c_void) -> bool;
#[cfg(target_os = "macos")]
type ModuleExitFn = unsafe extern "C" fn() -> bool;
#[cfg(target_os = "macos")]
const MODULE_ENTRY_SYM: &[u8] = b"bundleEntry\0";
#[cfg(target_os = "macos")]
const MODULE_EXIT_SYM: &[u8] = b"bundleExit\0";

/// A loaded VST3 shared object: its factory, its host context, and its lifetime.
///
/// Shared and reference-counted on the canonical module path, so two tracks
/// using the same plugin run the entry point once and the exit point once.
pub struct Vst3Module {
    /// `Option` only so `Drop` can release it *before* the exit hook runs.
    factory: Option<VstPtr<dyn IPluginFactory>>,
    /// Handed to `IPluginFactory3::set_host_context` as a raw pointer, so it has
    /// to outlive every instance. It used to be a local of `load()`, freed while
    /// the plugin still held the pointer.
    _host_app: Box<HostApplication>,
    module_exit: Option<ModuleExitFn>,
    path: PathBuf,
    /// MUST be last: the code stays mapped until after the factory is released
    /// and the exit hook has run.
    _lib: Library,
}

// The factory is only touched while loading, under the registry lock. The audio
// thread holds nothing but the `Arc`, which exists to keep the code mapped.
unsafe impl Send for Vst3Module {}
unsafe impl Sync for Vst3Module {}

impl Vst3Module {
    fn factory(&self) -> &VstPtr<dyn IPluginFactory> {
        self.factory.as_ref().expect("factory lives until Drop")
    }
    /// The `IHostApplication` pointer that `IPluginBase::initialize` expects.
    /// Passing null there is legal for a minimal host but many commercial
    /// plugins refuse to initialise on it.
    fn host_context(&self) -> *mut c_void {
        HostApplication::as_context(&self._host_app)
    }
    pub fn path(&self) -> &Path { &self.path }
}

impl Drop for Vst3Module {
    fn drop(&mut self) {
        // Exactly the reverse of the load order: release the factory, run the
        // module's exit hook, and only then let `_lib` unmap the code.
        self.factory = None;
        if let Some(exit) = self.module_exit {
            unsafe { exit() };
        }
    }
}

type ModuleRegistry = std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Weak<Vst3Module>>>;
static MODULES: std::sync::OnceLock<ModuleRegistry> = std::sync::OnceLock::new();

/// Load a plugin module, or hand back the one already loaded from that file.
pub fn load_module(path: &Path) -> Result<std::sync::Arc<Vst3Module>, String> {
    let module_path =
        resolve_module_path(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let key = std::fs::canonicalize(&module_path).unwrap_or(module_path);

    let registry = MODULES.get_or_init(Default::default);
    // A poisoned lock only means some other load panicked; the map itself is
    // still consistent, and refusing to load any plugin ever again is worse.
    let mut map = registry.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(live) = map.get(&key).and_then(std::sync::Weak::upgrade) {
        log::info!("VST3 module already loaded: {}", key.display());
        return Ok(live);
    }

    let lib = unsafe { Library::new(&key) }
        .map_err(|e| format!("dlopen {}: {e}", key.display()))?;

    // Entry point before the factory. A missing symbol is a warning, not an
    // error: the SDK's own host treats it as fatal, but being permissive here
    // costs nothing and loads more plugins.
    let module_exit: Option<ModuleExitFn> = unsafe {
        match lib.get::<ModuleEntryFn>(MODULE_ENTRY_SYM) {
            Ok(entry) => {
                #[cfg(target_os = "windows")]
                let ok = entry();
                #[cfg(not(target_os = "windows"))]
                let ok = entry(std::ptr::null_mut());
                if !ok {
                    return Err(format!(
                        "{}: module entry point refused to initialise",
                        key.display()
                    ));
                }
            }
            Err(_) => log::warn!("VST3 {}: no module entry point exported", key.display()),
        }
        lib.get::<ModuleExitFn>(MODULE_EXIT_SYM).ok().map(|s| *s)
    };

    // Built before the factory is fetched, so every early return from here on
    // unwinds through `Drop` and runs the exit hook.
    let mut module = Vst3Module {
        factory: None,
        _host_app: HostApplication::new(),
        module_exit,
        path: key.clone(),
        _lib: lib,
    };

    let factory_raw: *mut c_void = unsafe {
        let sym: Symbol<GetPluginFactoryFn> = module
            ._lib
            .get(b"GetPluginFactory\0")
            .map_err(|e| format!("GetPluginFactory not found: {e}"))?;
        sym()
    };
    if factory_raw.is_null() {
        return Err("GetPluginFactory returned null".to_string());
    }
    let factory = unsafe { VstPtr::<dyn IPluginFactory>::owned(factory_raw as *mut *mut _) }
        .ok_or_else(|| "null factory pointer".to_string())?;

    // Must be set before any `create_instance`, and must outlive the instances.
    if let Some(f3) = factory.cast::<dyn IPluginFactory3>() {
        let ctx = HostApplication::as_context(&module._host_app);
        unsafe { f3.set_host_context(ctx) };
        log::info!("VST3 load: host context set via IPluginFactory3");
    }
    module.factory = Some(factory);

    let arc = std::sync::Arc::new(module);
    map.insert(key, std::sync::Arc::downgrade(&arc));
    Ok(arc)
}

// ── Vst3Plugin ────────────────────────────────────────────────────────────────

/// Loaded and initialized VST3 instrument plugin.
///
/// IMPORTANT: `_module` is declared LAST — Rust drops fields in declaration
/// order, so the code stays mapped while all COM pointers are still being
/// dropped, and the module's exit hook runs only once the last instance is gone.
/// `ProcessContext::StatesAndFlags`, from the VST3 SDK.
///
/// A wire format: these are the bits the plugin reads to decide which
/// fields of the context it may believe. vst3-sys does not declare them, so
/// they are written out here against `ivstprocesscontext.h` and must not be
/// renumbered.
#[allow(dead_code)]
mod ctx_state {
    pub const PLAYING: u32 = 1 << 1;
    pub const CYCLE_ACTIVE: u32 = 1 << 2;
    pub const RECORDING: u32 = 1 << 3;
    pub const SYSTEM_TIME_VALID: u32 = 1 << 8;
    pub const PROJECT_TIME_MUSIC_VALID: u32 = 1 << 9;
    pub const TEMPO_VALID: u32 = 1 << 10;
    pub const BAR_POSITION_VALID: u32 = 1 << 11;
    pub const CYCLE_VALID: u32 = 1 << 12;
    pub const TIME_SIG_VALID: u32 = 1 << 13;
    pub const SMPTE_VALID: u32 = 1 << 14;
    pub const CLOCK_VALID: u32 = 1 << 15;
    pub const CONT_TIME_VALID: u32 = 1 << 17;
    pub const CHORD_VALID: u32 = 1 << 18;
}

pub struct Vst3Plugin {
    component:      VstPtr<dyn IComponent>,
    processor:      VstPtr<dyn IAudioProcessor>,
    controller:     Option<VstPtr<dyn IEditController>>,
    /// Where the plugin's own editor reports its parameter edits. The plugin
    /// holds a raw pointer to it, so it has to outlive the controller.
    component_handler: Option<Box<HostComponentHandler>>,

    host_event_list:  Box<HostEventList>,
    host_param_changes: Box<HostParamChanges>,
    pending_events:   Vec<Event>,
    /// (parameter id, sample offset, normalized value) for the coming block.
    pending_params:   Vec<(u32, i32, f64)>,

    /// The class's display name, straight from the factory.
    class_name: String,

    /// The class this instance was created from, as 32 ASCII hex characters.
    /// Stamped into the session so the track can be resolved again on a machine
    /// where the bundle sits somewhere else.
    class_id_hex: String,

    /// MIDI controller number -> parameter id, read once from `IMidiMapping`.
    /// Empty for a plugin that publishes no mapping, in which case controllers
    /// simply cannot be expressed in VST3 and are dropped.
    midi_cc_params: std::collections::HashMap<u8, u32>,

    out_l:      Vec<f32>,
    out_r:      Vec<f32>,
    in_l:       Vec<f32>,
    in_r:       Vec<f32>,

    /// True when the plugin accepted a stereo audio INPUT bus at load time
    /// (effect plugins). Instruments typically have 0 audio inputs and keep
    /// the historical (in:0, out:1) arrangement.
    has_audio_input: bool,
    /// Samples the plugin reports its output lags by.
    latency: u32,

    /// Where the transport stands, and the `ProcessContext` built from it.
    ///
    /// The context is kept rather than built per call because `ProcessData`
    /// takes a POINTER to it: it has to outlive the `process` call, and
    /// rebuilding it per block would be work for a structure that changes
    /// one field at a time.
    head: crate::plugin::PlayHead,
    context: ProcessContext,

    // MUST be last — the module must outlive all COM pointers above.
    _module: std::sync::Arc<Vst3Module>,
}

// Audio thread owns it exclusively; the Mutex in SequencerEngine serializes access.
unsafe impl Send for Vst3Plugin {}

impl Vst3Plugin {
    /// Load a VST3 plugin from a `.vst3` bundle or bare DLL path.
    pub fn load(path: &Path, sr: f32, block_size: usize) -> Result<Self, String> {
        Self::load_with_mode(path, sr, block_size, false)
    }

    /// As `load`, telling the plugin whether this instance renders OFFLINE.
    ///
    /// It matters, and a host that always says "real time" lies to every plugin
    /// that adapts to load. A bounce runs the engine flat out, far slower than
    /// real time by design, and an instrument that sheds voices when it cannot
    /// keep up will shed them through the whole render — which is heard as
    /// notes cut off, in a file that was supposed to be exact.
    pub fn load_with_mode(
        path: &Path,
        sr: f32,
        block_size: usize,
        offline: bool,
    ) -> Result<Self, String> {
        // Initialize COM on this thread. VST3 plugins (especially NI) call COM APIs during
        // construction. S_FALSE (0x1) = already initialized, which is fine to ignore.
        #[cfg(target_os = "windows")]
        unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_MULTITHREADED); }

        log::info!("VST3 load: resolving module in {}", path.display());
        // Module lifetime, factory and host context all belong to `Vst3Module`,
        // which is shared between instances of the same plugin.
        let module = load_module(path)?;
        let factory = module.factory();
        log::info!("VST3 load: module ready ({})", module.path().display());

        // Find first audio-effect class
        let class_count = unsafe { factory.count_classes() };
        let mut found = None;
        for i in 0..class_count {
            let mut info: PClassInfo = unsafe { std::mem::zeroed() };
            if unsafe { factory.get_class_info(i, &mut info) } != kResultOk { continue; }
            if category_is_audio_effect(&info.category) {
                found = Some((info.cid, char8_to_string(&info.name)));
                break;
            }
        }
        let (cid, class_name) =
            found.ok_or_else(|| "no audio-effect class in plugin".to_string())?;
        log::info!("VST3 load: found audio-effect class");

        // The host context was set on the factory when the module was loaded,
        // and it lives as long as the module — not as long as this function.
        log::info!("VST3 load: calling create_instance");

        // Create IComponent instance
        let icomponent_iid = <dyn IComponent as vst3_sys::ComInterface>::IID;
        let mut comp_raw: *mut c_void = std::ptr::null_mut();
        let r = unsafe {
            factory.create_instance(&cid, &icomponent_iid, &mut comp_raw)
        };
        if r != kResultOk || comp_raw.is_null() {
            return Err(format!("create_instance failed: {}", r));
        }
        let component = unsafe {
            VstPtr::<dyn IComponent>::owned(comp_raw as *mut *mut _)
                .ok_or_else(|| "null component".to_string())?
        };

        log::info!("VST3 load: component created");
        unsafe { component.initialize(module.host_context()) };
        log::info!("VST3 load: component initialized");

        // QI for IAudioProcessor
        let processor = component
            .cast::<dyn IAudioProcessor>()
            .ok_or_else(|| "plugin does not implement IAudioProcessor".to_string())?;

        // Setup processing
        let setup = ProcessSetup {
            // kRealtime = 0, kPrefetch = 1, kOffline = 2.
            process_mode:          if offline { 2 } else { 0 },
            symbolic_sample_size:  0, // kSample32
            max_samples_per_block: block_size as i32,
            sample_rate:           sr as f64,
        };
        log::info!("VST3 load: got IAudioProcessor");
        unsafe { processor.setup_processing(&setup) };
        log::info!("VST3 load: setup_processing done");

        // Set bus arrangements. Effect plugins expose an audio INPUT bus —
        // try (in: 1 stereo, out: 1 stereo) first, tolerantly: instruments
        // typically have 0 audio inputs, so if the component has no input
        // bus (or refuses the arrangement) fall back to the historical
        // (in: 0, out: 1 stereo) instrument arrangement. Record the result
        // so `process_effect` / `has_audio_input()` know which one is live.
        let num_audio_inputs = unsafe {
            component.get_bus_count(
                MediaTypes::kAudio as i32,
                BusDirections::kInput as i32,
            )
        };
        let mut has_audio_input = false;
        if num_audio_inputs > 0 {
            let mut in_stereo:  u64 = kStereo;
            let mut out_stereo: u64 = kStereo;
            let r = unsafe {
                processor.set_bus_arrangements(
                    &mut in_stereo,  1,
                    &mut out_stereo, 1,
                )
            };
            has_audio_input = r == kResultOk;
        }
        if !has_audio_input {
            let mut stereo: u64 = kStereo;
            unsafe {
                processor.set_bus_arrangements(
                    std::ptr::null_mut(), 0,
                    &mut stereo,          1,
                )
            };
        }
        log::info!("VST3 load: audio input bus = {}", has_audio_input);

        // Activate buses
        unsafe {
            component.activate_bus(
                MediaTypes::kEvent as i32,
                BusDirections::kInput as i32,
                0,
                1, // true
            )
        };
        if has_audio_input {
            unsafe {
                component.activate_bus(
                    MediaTypes::kAudio as i32,
                    BusDirections::kInput as i32,
                    0,
                    1, // true
                )
            };
        }
        unsafe {
            component.activate_bus(
                MediaTypes::kAudio as i32,
                BusDirections::kOutput as i32,
                0,
                1, // true
            )
        };
        log::info!("VST3 load: buses activated");
        unsafe { component.set_active(1) };
        log::info!("VST3 load: component active");
        unsafe { processor.set_processing(1) };
        log::info!("VST3 load: processing started");

        // Optional: QI for IEditController.
        //
        // This is a query_interface on the component, so when it succeeds the
        // controller IS the component — a "single component effect", which is
        // what every plugin we ship is. `IPluginBase::initialize` therefore
        // already ran for it on line above, and calling it again is calling it
        // twice on one object: forbidden by the VST3 layout, and the mirror of
        // the double `terminate` in `Drop` that used to segfault the bounce.
        //
        // A controller instantiated separately (`getControllerClassId` plus
        // `createInstance`) WOULD need its own initialize/terminate pair. If
        // that path is ever added, it must carry a flag saying the controller
        // is a distinct object, and both hooks come back with it.
        let controller = component.cast::<dyn IEditController>();

        let class_id_hex =
            String::from_utf8_lossy(&phonix_plugin::vstpreset::class_id_to_hex(&cid.data)).into_owned();

        log::info!("VST3 load: controller = {}", controller.is_some());

        // Read the controller-to-parameter map once. This is the only way a
        // MIDI controller can reach a VST3 plugin: there is no CC event, so a
        // sustain pedal is a parameter change on whatever id the plugin names
        // here. Bus 0, channel 0 — phonix sends one MIDI stream per track.
        let mut midi_cc_params = std::collections::HashMap::new();
        if let Some(mm) = controller.as_ref().and_then(|c| c.cast::<dyn IMidiMapping>()) {
            for cc in 0u8..CONTROLLER_COUNT {
                let mut pid: u32 = 0;
                let ok = unsafe { mm.get_midi_controller_assignment(0, 0, cc as i16, &mut pid) };
                if ok == kResultOk { midi_cc_params.insert(cc, pid); }
            }
        }
        log::info!("VST3 load: {} MIDI controllers mapped", midi_cc_params.len());

        let host_event_list  = HostEventList::new();
        let host_param_changes = HostParamChanges::new();

        log::info!("VST3 load: complete, returning plugin");
        // Reported once the processor is set up; refreshed on restart.
        let latency = unsafe { processor.get_latency_samples() };
        Ok(Self {
            latency,
            component,
            processor,
            controller,
            component_handler: None,
            host_event_list,
            host_param_changes,
            pending_events: Vec::new(),
            pending_params:  Vec::new(),
            midi_cc_params,
            out_l: vec![0.0f32; block_size],
            out_r: vec![0.0f32; block_size],
            in_l:  vec![0.0f32; block_size],
            in_r:  vec![0.0f32; block_size],
            has_audio_input,
            head: Default::default(),
            context: ProcessContext::default(),
            class_name,
            class_id_hex,
            _module: module,
        })
    }

    /// True when the plugin accepted a stereo audio input bus at load
    /// (i.e. it can be used as an insert EFFECT via `process_effect`).
    pub fn has_audio_input(&self) -> bool {
        self.has_audio_input
    }

    /// Where the transport stands for the coming block.
    ///
    /// Stored, not sent: VST3 has no transport message. The host hands the
    /// plugin a `ProcessContext` alongside the audio on every `process`, and
    /// this is what that structure is filled from. Until it was, `context`
    /// was a null pointer and every hosted VST3 ran free -- no tempo, no
    /// beat, no playhead -- which a tempo-synced delay or an arpeggiator
    /// inside the plugin has no way to work around.
    pub fn set_transport(&mut self, head: &crate::plugin::PlayHead) {
        if self.head == *head {
            return;
        }
        self.head = *head;
        let c = &mut self.context;
        c.sample_rate = head.sample_rate;
        c.project_time_samples = head.samples;
        c.continuous_time_samples = head.continuous_samples;
        c.project_time_music = head.beats;
        c.bar_position_music = head.bar_start_beats;
        c.tempo = head.tempo;
        c.time_sig_num = head.time_sig.0 as i32;
        c.time_sig_den = head.time_sig.1.max(1) as i32;
        let mut state = ctx_state::TEMPO_VALID
            | ctx_state::TIME_SIG_VALID
            | ctx_state::PROJECT_TIME_MUSIC_VALID
            | ctx_state::BAR_POSITION_VALID
            | ctx_state::CONT_TIME_VALID;
        if head.playing { state |= ctx_state::PLAYING; }
        if head.recording { state |= ctx_state::RECORDING; }
        if let Some((start, end)) = head.loop_beats {
            c.cycle_start_music = start;
            c.cycle_end_music = end;
            state |= ctx_state::CYCLE_ACTIVE | ctx_state::CYCLE_VALID;
        }
        c.state = state;
    }

    /// Move this block's parameter changes into the queue the plugin reads.
    ///
    /// The queue is the only path that reaches the PROCESSOR, and the only way a
    /// MIDI controller can be expressed in VST3 at all. The controller is still
    /// notified so the plugin's own view of its state follows; that call really
    /// belongs off the audio thread, but it is what this host already did and
    /// removing it here would change more than the pedal.
    fn flush_param_changes(&mut self) {
        HostParamChanges::clear(&mut self.host_param_changes);
        // `take` hands the buffer back afterwards, so this stops allocating once
        // the set of touched parameters settles.
        let mut pending = std::mem::take(&mut self.pending_params);
        for (id, offset, val) in pending.drain(..) {
            HostParamChanges::push(&mut self.host_param_changes, id, offset, val);
            if let Some(ref ctrl) = self.controller {
                unsafe { ctrl.set_param_normalized(id, val) };
            }
        }
        self.pending_params = pending;
    }

    /// Queue a MIDI controller for the coming block.
    ///
    /// Returns false when the plugin publishes no mapping for that controller,
    /// which is not an error: VST3 simply has no way to carry it.
    pub fn send_cc(&mut self, cc: u8, value: u8, sample_offset: i32) -> bool {
        let Some(&id) = self.midi_cc_params.get(&cc) else { return false };
        self.pending_params.push((id, sample_offset, value as f64 / 127.0));
        true
    }

    /// The pitch wheel, 14 bits, 8192 at rest.
    pub fn pitch_bend(&mut self, value: u16, sample_offset: i32) -> bool {
        let Some(&id) = self.midi_cc_params.get(&PITCH_BEND) else { return false };
        self.pending_params.push((id, sample_offset, value.min(16_383) as f64 / 16_383.0));
        true
    }

    /// Channel pressure, the aftertouch a keyboard sends for the whole
    /// channel rather than per key.
    pub fn channel_pressure(&mut self, value: u8, sample_offset: i32) -> bool {
        let Some(&id) = self.midi_cc_params.get(&AFTERTOUCH) else { return false };
        self.pending_params.push((id, sample_offset, (value & 0x7F) as f64 / 127.0));
        true
    }

    /// True when the plugin can be sent this controller at all.
    pub fn maps_cc(&self, cc: u8) -> bool { self.midi_cc_params.contains_key(&cc) }

    /// The class id this instance was created from, 32 ASCII hex characters.
    pub fn class_id_hex(&self) -> &str { &self.class_id_hex }

    /// Render `num_frames` frames into `stereo_out` (interleaved L/R).
    pub fn process(&mut self, stereo_out: &mut [f32], num_frames: usize) {
        if num_frames == 0 { return; }

        // Ensure scratch buffers are large enough
        if self.out_l.len() < num_frames { self.out_l.resize(num_frames, 0.0); }
        if self.out_r.len() < num_frames { self.out_r.resize(num_frames, 0.0); }
        self.out_l[..num_frames].fill(0.0);
        self.out_r[..num_frames].fill(0.0);

        // Fill event list from pending_events
        HostEventList::clear(&mut self.host_event_list);
        for e in self.pending_events.drain(..) {
            HostEventList::push_event(&mut self.host_event_list, e);
        }

        self.flush_param_changes();

        let el_ptr     = HostEventList::as_static(&mut self.host_event_list);
        let params_ptr = HostParamChanges::as_static(&mut self.host_param_changes);
        // output_param_changes and output_events: plugins should tolerate null here
        let null_params_out: StaticVstPtr<dyn IParameterChanges> =
            unsafe { std::mem::transmute(std::ptr::null_mut::<c_void>()) };
        let null_events_out: StaticVstPtr<dyn IEventList> =
            unsafe { std::mem::transmute(std::ptr::null_mut::<c_void>()) };

        let l_ptr = self.out_l.as_mut_ptr() as *mut c_void;
        let r_ptr = self.out_r.as_mut_ptr() as *mut c_void;
        let mut channel_ptrs: [*mut c_void; 2] = [l_ptr, r_ptr];
        let mut output_bus = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: channel_ptrs.as_mut_ptr(),
        };

        let mut data = ProcessData {
            process_mode:          0, // kRealtime
            symbolic_sample_size:  0, // kSample32
            num_samples:           num_frames as i32,
            num_inputs:            0,
            num_outputs:           1,
            inputs:                std::ptr::null_mut(),
            outputs:               &mut output_bus,
            input_param_changes:   params_ptr,
            output_param_changes:  null_params_out,
            input_events:          el_ptr,
            output_events:         null_events_out,
            // The transport, as `set_transport` last left it. A null here is
            // what every hosted VST3 used to get.
            context:               &mut self.context,
        };

        unsafe { self.processor.process(&mut data) };

        // Interleave L/R into stereo_out
        let n = num_frames.min(stereo_out.len() / 2);
        for i in 0..n {
            stereo_out[i * 2]     = self.out_l[i];
            stereo_out[i * 2 + 1] = self.out_r[i];
        }
    }

    /// Process `io` (interleaved stereo) THROUGH the plugin as an insert
    /// effect: deinterleave into the input bus scratch, run the processor
    /// with 1 input + 1 output bus, interleave the output back into `io`.
    /// If the plugin has no audio input bus, `io` is left untouched
    /// (pass-through). The instrument `process()` above is unchanged.
    pub fn process_effect(&mut self, io: &mut [f32], num_frames: usize) {
        if num_frames == 0 || !self.has_audio_input { return; }
        let num_frames = num_frames.min(io.len() / 2);

        // Ensure scratch buffers are large enough (steady state: no alloc)
        if self.in_l.len()  < num_frames { self.in_l.resize(num_frames, 0.0); }
        if self.in_r.len()  < num_frames { self.in_r.resize(num_frames, 0.0); }
        if self.out_l.len() < num_frames { self.out_l.resize(num_frames, 0.0); }
        if self.out_r.len() < num_frames { self.out_r.resize(num_frames, 0.0); }

        // Deinterleave io -> input channel scratch
        for i in 0..num_frames {
            self.in_l[i] = io[i * 2];
            self.in_r[i] = io[i * 2 + 1];
        }
        self.out_l[..num_frames].fill(0.0);
        self.out_r[..num_frames].fill(0.0);

        // No MIDI for insert effects — pass an EMPTY event list (plugins may
        // dereference input_events unconditionally, so never pass null).
        HostEventList::clear(&mut self.host_event_list);

        self.flush_param_changes();

        let el_ptr     = HostEventList::as_static(&mut self.host_event_list);
        let params_ptr = HostParamChanges::as_static(&mut self.host_param_changes);
        let null_params_out: StaticVstPtr<dyn IParameterChanges> =
            unsafe { std::mem::transmute(std::ptr::null_mut::<c_void>()) };
        let null_events_out: StaticVstPtr<dyn IEventList> =
            unsafe { std::mem::transmute(std::ptr::null_mut::<c_void>()) };

        let mut in_ptrs: [*mut c_void; 2] = [
            self.in_l.as_mut_ptr() as *mut c_void,
            self.in_r.as_mut_ptr() as *mut c_void,
        ];
        let mut input_bus = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: in_ptrs.as_mut_ptr(),
        };
        let mut out_ptrs: [*mut c_void; 2] = [
            self.out_l.as_mut_ptr() as *mut c_void,
            self.out_r.as_mut_ptr() as *mut c_void,
        ];
        let mut output_bus = AudioBusBuffers {
            num_channels: 2,
            silence_flags: 0,
            buffers: out_ptrs.as_mut_ptr(),
        };

        let mut data = ProcessData {
            process_mode:          0, // kRealtime
            symbolic_sample_size:  0, // kSample32
            num_samples:           num_frames as i32,
            num_inputs:            1,
            num_outputs:           1,
            inputs:                &mut input_bus,
            outputs:               &mut output_bus,
            input_param_changes:   params_ptr,
            output_param_changes:  null_params_out,
            input_events:          el_ptr,
            output_events:         null_events_out,
            // An insert wants the transport as much as an instrument does:
            // a tempo-synced delay is the commonest effect there is.
            context:               &mut self.context,
        };

        unsafe { self.processor.process(&mut data) };

        // Interleave the OUTPUT back into io
        for i in 0..num_frames {
            io[i * 2]     = self.out_l[i];
            io[i * 2 + 1] = self.out_r[i];
        }
    }

    pub fn note_on(&mut self, note: u8, velocity: u8) {
        let e = Event {
            bus_index:    0,
            sample_offset: 0,
            ppq_position: 0.0,
            flags:        1, // kIsLive
            type_:        EventTypes::kNoteOnEvent as u16,
            event: EventData {
                note_on: NoteOnEvent {
                    channel:  0,
                    pitch:    note as i16,
                    tuning:   0.0,
                    velocity: velocity as f32 / 127.0,
                    length:   0,
                    note_id:  -1,
                },
            },
        };
        self.pending_events.push(e);
    }

    pub fn note_off(&mut self, note: u8) {
        let e = Event {
            bus_index:    0,
            sample_offset: 0,
            ppq_position: 0.0,
            flags:        1, // kIsLive
            type_:        EventTypes::kNoteOffEvent as u16,
            event: EventData {
                note_off: NoteOffEvent {
                    channel:  0,
                    pitch:    note as i16,
                    velocity: 0.0,
                    note_id:  -1,
                    tuning:   0.0,
                },
            },
        };
        self.pending_events.push(e);
    }

    pub fn set_param(&mut self, id: u32, value: f64) {
        // A knob turn has no meaningful position inside the block, so offset 0.
        self.pending_params.push((id, 0, value));
    }

    pub fn get_param(&self, id: u32) -> f64 {
        if let Some(ref ctrl) = self.controller {
            unsafe { ctrl.get_param_normalized(id) }
        } else {
            0.0
        }
    }

    pub fn param_count(&self) -> i32 {
        if let Some(ref ctrl) = self.controller {
            unsafe { ctrl.get_parameter_count() }
        } else {
            0
        }
    }

    pub fn param_info(&self, idx: i32) -> Option<ParameterInfo> {
        let ctrl = self.controller.as_ref()?;
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        let r = unsafe { ctrl.get_parameter_info(idx, &mut info) };
        if r == kResultOk { Some(info) } else { None }
    }

    /// Save plugin state to a byte blob.
    pub fn get_state(&self) -> Vec<u8> {
        let mut stream = HostStream::write_stream();
        let shared = HostStream::as_shared(&mut stream);
        unsafe { self.component.get_state(shared) };
        HostStream::into_bytes(stream)
    }

    /// Restore plugin state from a byte blob.
    pub fn set_state(&mut self, data: &[u8]) {
        if data.is_empty() { return; }
        let mut stream = HostStream::read_stream(data.to_vec());
        let shared = HostStream::as_shared(&mut stream);
        unsafe { self.component.set_state(shared) };
        // Also push state to controller so its parameters reflect the loaded state
        if let Some(ref ctrl) = self.controller {
            let mut stream2 = HostStream::read_stream(data.to_vec());
            let shared2 = HostStream::as_shared(&mut stream2);
            unsafe { ctrl.set_component_state(shared2) };
        }
    }

    /// A handle on this plugin's edit controller for the GUI thread, so the
    /// plugin's own editor can be opened in a window of ours.
    ///
    /// The plugin itself lives on the audio thread and must stay there. What
    /// crosses over is one COM reference (an AddRef, nothing more) plus an
    /// `Arc` on the loaded module, so the code the view runs cannot be
    /// unmapped while the window is open.
    pub fn editor_conn(&self) -> Option<Vst3EditorConn> {
        self.controller.as_ref().map(|c| Vst3EditorConn {
            ctrl: c.clone(),
            _module: self._module.clone(),
        })
    }

    /// Give the plugin somewhere to report its own edits.
    ///
    /// Not optional in practice: a plugin whose editor writes parameters
    /// through the host (which is the normal path, and the only one for
    /// nice-plug's own widgets) is INERT without a handler. Held for the
    /// lifetime of the plugin, because the plugin keeps the pointer.
    pub fn set_component_handler(&mut self, handler: Box<HostComponentHandler>) {
        if let Some(ref ctrl) = self.controller {
            let ptr = HostComponentHandler::as_shared(&handler);
            unsafe { ctrl.set_component_handler(ptr) };
        }
        self.component_handler = Some(handler);
    }

    /// Build (or refresh) the param cache from the controller.
    /// The plugin's reported latency in samples.
    pub fn latency_samples(&self) -> usize { self.latency as usize }

    /// Re-read the latency after the plugin asked for a restart.
    pub fn refresh_latency(&mut self) {
        self.latency = unsafe { self.processor.get_latency_samples() };
    }

    pub fn build_param_cache(&self) -> Vst3ParamCache {
        let mut cache = Vst3ParamCache::default();
        let ctrl = match self.controller.as_ref() {
            Some(c) => c,
            None    => return cache,
        };
        cache.plugin_name = self.class_name.clone();

        // The plugin's own rendering of a value. `get_param_string_by_value` is
        // the only thing that knows a 0.5 means "Lowpass 24" or "-6.0 dB".
        let display_of = |id: u32, v: f64| -> String {
            let mut buf: [i16; 128] = [0; 128];
            if unsafe { ctrl.get_param_string_by_value(id, v, buf.as_mut_ptr()) } == kResultOk {
                string128_to_string(&buf)
            } else {
                String::new()
            }
        };

        let count = unsafe { ctrl.get_parameter_count() };
        for i in 0..count {
            let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
            if unsafe { ctrl.get_parameter_info(i, &mut info) } != kResultOk { continue; }
            // Skip what the plugin marks hidden. A plugin that accepts MIDI
            // controllers publishes one parameter per controller per channel —
            // 2080 of them — all flagged hidden precisely so a host does not
            // show them. Without this the parameter grid is unusable.
            if info.flags & PARAM_IS_HIDDEN != 0 { continue; }

            let value = unsafe { ctrl.get_param_normalized(info.id) };

            // A list parameter's positions are evenly spaced in the normalized
            // range, so each label can be asked for by its own value.
            let mut enum_labels = Vec::new();
            if info.flags & (ParameterFlags::kIsList as i32) != 0 && info.step_count > 0 {
                for k in 0..=info.step_count {
                    enum_labels.push(display_of(info.id, k as f64 / info.step_count as f64));
                }
            }

            cache.params.push(Vst3ParamEntry {
                id: info.id,
                title: string128_to_string(&info.title),
                short_title: string128_to_string(&info.short_title),
                units: string128_to_string(&info.units),
                step_count: info.step_count,
                value,
                default: info.default_normalized_value,
                flags: info.flags,
                display: display_of(info.id, value),
                enum_labels,
            });
        }
        cache
    }
}

impl Drop for Vst3Plugin {
    fn drop(&mut self) {
        // Hand the component handler back BEFORE anything else. We allocated
        // it, we own it in `self.component_handler`, and the plugin holds the
        // bare pointer for as long as it is set — the SDK's contract is that
        // the host clears it before the object it points at goes away.
        //
        // Skipping this is what segfaulted every bounce. Dropping this struct
        // freed the handler's box while the plugin still pointed at it; the
        // plugin's own destructor, which runs later on whichever `Release` is
        // last (in a bounce, the one `Vst3EditorConn` holds in the sequencer's
        // GUI handles), then released a handler that was no longer there and
        // jumped through a dead vtable.
        if self.component_handler.is_some() {
            if let Some(ref ctrl) = self.controller {
                unsafe { ctrl.set_component_handler(HostComponentHandler::none()) };
            }
        }
        unsafe {
            self.processor.set_processing(0);
            self.component.set_active(0);
            self.component.terminate();
        }
        // No second `terminate` for `self.controller`: it is the component,
        // obtained by query_interface (see `load`). Terminating it twice tore
        // down the plugin's state twice, and the crash surfaced later, on
        // whichever `Release` happened to be last — in a bounce, the one held
        // by `Vst3EditorConn` in `EngineGuiHandle`, long after the WAV had
        // been written.
        // VstPtrs (component, processor, controller) are dropped first (Release
        // called), then `_module` last. If this was the final instance the module
        // releases the factory, runs its exit hook and unmaps the code, in that
        // order.
    }
}

// ── Platform layout ───────────────────────────────────────────────────────────
//
// A `.vst3` bundle is a directory: `<Name>.vst3/Contents/<arch>/<Name>.<ext>`.
// `scripts/build_plugins.sh` already emits exactly this for both targets we
// build; only the host used to be hard-wired to the Windows arch, which made
// every Linux bundle invisible to the scanner AND unloadable.

/// Architecture directory inside a bundle.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub const VST3_ARCH_DIR: &str = "x86_64-linux";
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub const VST3_ARCH_DIR: &str = "aarch64-linux";
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
pub const VST3_ARCH_DIR: &str = "x86_64-win";
#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
pub const VST3_ARCH_DIR: &str = "arm64-win";
#[cfg(target_os = "macos")]
pub const VST3_ARCH_DIR: &str = "MacOS";

/// Extension of the loadable module inside the bundle.
#[cfg(target_os = "linux")]
pub const VST3_MODULE_EXT: &str = "so";
#[cfg(target_os = "windows")]
pub const VST3_MODULE_EXT: &str = "vst3";
/// macOS modules carry no extension, and loading one needs `CFBundle` rather
/// than a plain `dlopen`, so discovery works there but `load` does not yet.
#[cfg(target_os = "macos")]
pub const VST3_MODULE_EXT: &str = "";

/// Every standard VST3 search path for this platform, in scan order.
///
/// Lives here rather than in the GUI because the headless render path needs it
/// too: a bounce cannot ask a window where the plugins are.
pub fn default_vst3_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    // Explicit override first. This is how a development tree, a test, or a CI
    // run points the host at bundles that were never installed system-wide.
    if let Ok(p) = std::env::var("PHONIX_VST3_PATH") {
        dirs.extend(std::env::split_paths(&p));
    }

    #[cfg(target_os = "windows")]
    {
        for var in ["CommonProgramFiles", "CommonProgramFiles(x86)"] {
            if let Ok(v) = std::env::var(var) { dirs.push(PathBuf::from(v).join("VST3")); }
        }
        // Where `install_vst3.ps1 -User` puts them, and where the host has never
        // looked: a per-user install was simply invisible.
        if let Ok(v) = std::env::var("LOCALAPPDATA") {
            dirs.push(PathBuf::from(v).join("Programs").join("Common").join("VST3"));
        }
        if let Ok(v) = std::env::var("USERPROFILE") { dirs.push(PathBuf::from(v).join(".vst3")); }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") { dirs.push(PathBuf::from(home).join(".vst3")); }
        dirs.push(PathBuf::from("/usr/lib/vst3"));
        dirs.push(PathBuf::from("/usr/local/lib/vst3"));
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
        }
        dirs.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
    }

    // Beside the executable, and the repository's own bundle output, so a
    // freshly built plugin is findable without an install step.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() { dirs.push(d.join("vst3")); }
    }
    dirs.push(PathBuf::from("target").join("bundled").join(std::env::consts::OS));

    dirs.retain(|d| d.is_dir());
    dirs.dedup();
    dirs
}

// ── Scan / discovery ──────────────────────────────────────────────────────────

/// Walk a VST3 directory and enumerate plugins by filesystem path only.
///
/// No DLLs are loaded during the scan — names come from the bundle/file stem.
/// This avoids crashes from badly-behaved plugins that fail to load cleanly
/// without a full host environment.
pub fn scan_vst3_dir(dir: &Path) -> Vec<Vst3PluginInfo> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    scan_into(dir, 0, &mut out, &mut seen);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Every standard search path for this platform, in scan order.
pub fn scan_all() -> Vec<Vst3PluginInfo> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in default_vst3_dirs() {
        scan_into(&dir, 0, &mut out, &mut seen);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Locate an installed bundle by its display name (the bundle's file stem).
///
/// This is how a headless render finds a plugin it needs without a hard-coded
/// path: the same session then loads on a machine that installed it elsewhere.
pub fn find_by_name(name: &str) -> Option<PathBuf> {
    scan_all()
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| p.path)
}

/// Every class a bundle publishes, as 32 ASCII hex characters each.
///
/// Reads the factory's class list, which needs the module loaded but NOT any
/// instance created — much cheaper and much safer than instantiating.
pub fn class_ids_of(path: &Path) -> Vec<String> {
    let Ok(module) = load_module(path) else { return Vec::new() };
    let factory = module.factory();
    let count = unsafe { factory.count_classes() };
    let mut out = Vec::new();
    for i in 0..count {
        let mut info: PClassInfo = unsafe { std::mem::zeroed() };
        if unsafe { factory.get_class_info(i, &mut info) } != kResultOk { continue; }
        out.push(String::from_utf8_lossy(&phonix_plugin::vstpreset::class_id_to_hex(&info.cid.data))
            .into_owned());
    }
    out
}

/// Find an installed bundle by the VST3 class id a session recorded.
///
/// The stored path is machine-specific — a project moved to another machine, or
/// restored from a backup, points at a bundle that is not there. The class id
/// survives that. Recovery costs a scan that opens each candidate's factory, so
/// the answer is cached for the life of the process: once per machine, not once
/// per track.
pub fn resolve_by_class_id(hex: &str) -> Option<PathBuf> {
    static BY_CLASS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, PathBuf>>,
    > = std::sync::OnceLock::new();
    let cache = BY_CLASS.get_or_init(Default::default);

    if let Ok(map) = cache.lock() {
        if let Some(p) = map.get(hex) { return Some(p.clone()); }
    }
    for info in scan_all() {
        for found in class_ids_of(&info.path) {
            if found.eq_ignore_ascii_case(hex) {
                log::info!("VST3: class {hex} resolved to {}", info.path.display());
                if let Ok(mut map) = cache.lock() {
                    map.insert(hex.to_string(), info.path.clone());
                }
                return Some(info.path);
            }
        }
    }
    log::warn!("VST3: no installed bundle publishes class {hex}");
    None
}

/// Load what a session asked for: the recorded path first, and the class id as
/// the recovery route when that path is gone.
///
/// Returns the plugin and the path it actually came from, so the caller can
/// repoint the session and make the next load an O(1) one again.
pub fn load_for_session(
    path: &Path,
    class_id: Option<&str>,
    sr: f32,
    block_size: usize,
    offline: bool,
) -> Result<(Vst3Plugin, PathBuf), String> {
    let direct = if path.as_os_str().is_empty() {
        Err("no path recorded".to_string())
    } else {
        Vst3Plugin::load_with_mode(path, sr, block_size, offline).map(|p| (p, path.to_path_buf()))
    };
    match (direct, class_id) {
        (Ok(ok), _) => Ok(ok),
        (Err(first), Some(hex)) => {
            let found = resolve_by_class_id(hex)
                .ok_or_else(|| format!("{first}; and no installed plugin has class {hex}"))?;
            Vst3Plugin::load_with_mode(&found, sr, block_size, offline).map(|p| (p, found))
        }
        (Err(first), None) => Err(first),
    }
}

/// Vendors group their bundles into sub-directories of a search path, so a flat
/// `read_dir` misses most of a real installation. Bounded depth, and never
/// descend into a `.vst3` — that directory *is* the bundle.
fn scan_into(
    dir: &Path,
    depth: u32,
    out: &mut Vec<Vst3PluginInfo>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    const MAX_DEPTH: u32 = 4;
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_bundle = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("vst3"));
        if is_bundle {
            let Ok(module) = resolve_module_path(&path) else { continue };
            // One plugin symlinked into two search paths is one plugin.
            let key = std::fs::canonicalize(&module).unwrap_or(module);
            if !seen.insert(key) { continue; }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() { continue; }
            // The factory's class list, which needs the module loaded but no
            // instance created: the same cost the CLAP scan pays for its
            // descriptors. A bundle that fails to open keeps its stem and
            // no category.
            let mut info = Vst3PluginInfo {
                name, vendor: String::new(), path,
                subcategories: Vec::new(), is_instrument: false, is_effect: false,
            };
            if let Some(c) = describe_bundle(&info.path) {
                info.vendor = c.vendor;
                info.is_instrument = c.subcategories.iter().any(|s| s == "Instrument");
                info.is_effect = c.subcategories.iter().any(|s| s == "Fx");
                info.subcategories = c.subcategories;
            }
            out.push(info);
        } else if depth < MAX_DEPTH && path.is_dir() {
            scan_into(&path, depth + 1, out, seen);
        }
    }
}

/// The first audio-effect class a bundle's factory declares.
#[derive(Clone, Debug)]
pub struct Vst3ClassInfo {
    pub name: String,
    pub vendor: String,
    /// Split on the `|` the factory joins them with.
    pub subcategories: Vec<String>,
}

/// Read the factory's class list through `IPluginFactory2`, which carries
/// the vendor and the sub-categories `IPluginFactory` does not.
///
/// Answered from a cache after the first look: a scan runs on every open of
/// the track picker, and loading a plugin's code to ask its name again each
/// time is both slow and a lifetime hazard for a plugin whose exit hook does
/// more than it should.
pub fn describe_bundle(path: &Path) -> Option<Vst3ClassInfo> {
    static SEEN: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, Option<Vst3ClassInfo>>>,
    > = std::sync::OnceLock::new();
    let cache = SEEN.get_or_init(Default::default);
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(&key) {
            return hit.clone();
        }
    }
    let found = describe_bundle_uncached(path);
    if let Ok(mut map) = cache.lock() {
        map.insert(key, found.clone());
    }
    found
}

fn describe_bundle_uncached(path: &Path) -> Option<Vst3ClassInfo> {
    let module = load_module(path).ok()?;
    let factory = module.factory();
    let count = unsafe { factory.count_classes() };
    if let Some(f2) = factory.cast::<dyn IPluginFactory2>() {
        for i in 0..count {
            let mut info: PClassInfo2 = unsafe { std::mem::zeroed() };
            if unsafe { f2.get_class_info2(i, &mut info) } != kResultOk { continue; }
            if !category_is_audio_effect(&info.category) { continue; }
            let subs = char8_to_string_n(&info.subcategories);
            return Some(Vst3ClassInfo {
                name: char8_to_string(&info.name),
                vendor: char8_to_string(&info.vendor),
                subcategories: subs.split('|').filter(|s| !s.is_empty()).map(str::to_string).collect(),
            });
        }
    }
    for i in 0..count {
        let mut info: PClassInfo = unsafe { std::mem::zeroed() };
        if unsafe { factory.get_class_info(i, &mut info) } != kResultOk { continue; }
        if !category_is_audio_effect(&info.category) { continue; }
        return Some(Vst3ClassInfo {
            name: char8_to_string(&info.name),
            vendor: String::new(),
            subcategories: Vec::new(),
        });
    }
    None
}

/// Why a bundle could not be resolved.
///
/// `WrongArch` exists because "not found" is a useless diagnosis for a bundle
/// that is correctly installed for another architecture — which is exactly what
/// every Windows plugin looks like to a Linux host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    NotFound,
    WrongArch { wanted: &'static str, found: Vec<String> },
}

impl std::fmt::Display for BundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BundleError::NotFound => write!(f, "no VST3 module inside the bundle"),
            BundleError::WrongArch { wanted, found } => write!(
                f,
                "bundle carries {} but this host needs {wanted}",
                found.join(", ")
            ),
        }
    }
}

/// Resolve the loadable module from a `.vst3` directory bundle, or accept a bare
/// shared object.
pub fn resolve_module_path(path: &Path) -> Result<PathBuf, BundleError> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(BundleError::NotFound);
    }
    let contents = path.join("Contents");
    let arch = contents.join(VST3_ARCH_DIR);
    if arch.is_dir() {
        // The layout names the module after the bundle; prefer that, and settle
        // for the first file with the right extension otherwise.
        if let Some(stem) = path.file_stem() {
            let preferred = arch.join(stem).with_extension(VST3_MODULE_EXT);
            if preferred.is_file() {
                return Ok(preferred);
            }
        }
        if let Ok(entries) = std::fs::read_dir(&arch) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file()
                    && p.extension().and_then(|x| x.to_str()) == Some(VST3_MODULE_EXT)
                {
                    return Ok(p);
                }
            }
        }
    }
    let mut found: Vec<String> = std::fs::read_dir(&contents)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    if found.is_empty() {
        return Err(BundleError::NotFound);
    }
    found.sort();
    Err(BundleError::WrongArch { wanted: VST3_ARCH_DIR, found })
}


// ── Helpers ───────────────────────────────────────────────────────────────────

/// A factory string: fixed-size, NUL-padded ASCII.
fn char8_to_string(raw: &[char8; 64]) -> String {
    char8_to_string_n(raw)
}

fn char8_to_string_n(raw: &[char8]) -> String {
    let bytes: Vec<u8> = raw.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn category_is_audio_effect(category: &[char8; 32]) -> bool {
    let target = b"Audio Module Class";
    for (i, &b) in target.iter().enumerate() {
        if i >= 32 { break; }
        if category[i] as u8 != b { return false; }
    }
    true
}


fn string128_to_string(arr: &[i16; 128]) -> String {
    let chars: Vec<u16> = arr.iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u16)
        .collect();
    String::from_utf16_lossy(&chars)
}

// ── Tests ─────────────────────────────────────────────────────────────────────
//
// Filesystem only: no plugin is loaded, so these run anywhere and need nothing
// installed. They cover the part that made every Linux bundle invisible.

#[cfg(test)]
mod tests {
    use crate::plugin::PlayHead;

    /// The controller sweep covers the two numbers VST3 puts past the MIDI
    /// ones. Stopping at 127, as it did, left a pitch wheel and channel
    /// pressure with no parameter to travel on and no way into a plugin.
    #[test]
    fn the_sweep_reaches_the_wheel_and_the_pressure() {
        use super::{AFTERTOUCH, CONTROLLER_COUNT, PITCH_BEND};
        assert!(AFTERTOUCH < CONTROLLER_COUNT);
        assert!(PITCH_BEND < CONTROLLER_COUNT);
        // And it stops there: the numbering ends at the pitch wheel.
        assert_eq!(CONTROLLER_COUNT, PITCH_BEND + 1);
    }

    /// The bits a plugin reads to know which fields of the context it may
    /// believe. They are the VST3 SDK's own numbering and a wire format; a
    /// renumbering here would have every hosted plugin believe the wrong
    /// halves of the transport.
    #[test]
    fn the_context_state_bits_are_the_sdks() {
        use super::ctx_state as f;
        assert_eq!(f::PLAYING, 2);
        assert_eq!(f::CYCLE_ACTIVE, 4);
        assert_eq!(f::RECORDING, 8);
        assert_eq!(f::SYSTEM_TIME_VALID, 256);
        assert_eq!(f::PROJECT_TIME_MUSIC_VALID, 512);
        assert_eq!(f::TEMPO_VALID, 1024);
        assert_eq!(f::BAR_POSITION_VALID, 2048);
        assert_eq!(f::CYCLE_VALID, 4096);
        assert_eq!(f::TIME_SIG_VALID, 8192);
        assert_eq!(f::SMPTE_VALID, 16384);
        assert_eq!(f::CLOCK_VALID, 32768);
        assert_eq!(f::CONT_TIME_VALID, 131_072);
        assert_eq!(f::CHORD_VALID, 262_144);
    }

    /// A hosted VST3 is handed the transport, and it says so.
    ///
    /// Every hosted plugin used to get a null `ProcessContext`: no tempo, no
    /// beat, no playhead, which a tempo-synced delay or an arpeggiator inside
    /// the plugin cannot work around. This needs a bundle to load, so it is
    /// skipped where none is installed; what it asserts is the mapping from
    /// the sequencer's playhead onto the structure the plugin reads.
    #[test]
    fn a_hosted_plugin_is_told_where_the_transport_is() {
        let Some(path) = super::find_by_name("Cordis") else {
            eprintln!("Cordis is not installed; skipping");
            return;
        };
        let mut p = match super::Vst3Plugin::load(&path, 48_000.0, 128) {
            Ok(p) => p,
            Err(e) => { eprintln!("Cordis did not load ({e}); skipping"); return }
        };

        // Stopped, at the top: the plugin is told the tempo and the metre,
        // and that nothing is playing.
        let mut head = PlayHead {
            tempo: 132.0, playing: false, recording: false,
            beats: 0.0, bar_start_beats: 0.0, samples: 0, continuous_samples: 0,
            time_sig: (7, 8), loop_beats: None, sample_rate: 48_000.0,
        };
        p.set_transport(&head);
        assert_eq!(p.context.tempo, 132.0);
        assert_eq!(p.context.time_sig_num, 7);
        assert_eq!(p.context.time_sig_den, 8);
        assert_eq!(p.context.state & super::ctx_state::PLAYING, 0);
        assert_ne!(p.context.state & super::ctx_state::TEMPO_VALID, 0);
        assert_ne!(p.context.state & super::ctx_state::TIME_SIG_VALID, 0);
        assert_eq!(p.context.state & super::ctx_state::CYCLE_ACTIVE, 0,
                   "a cycle was announced with no loop running");

        // Playing, three and a half bars in, inside a loop.
        head.playing = true;
        head.recording = true;
        head.beats = 14.5;
        head.bar_start_beats = 14.0;
        head.samples = 316_000;
        head.continuous_samples = 999;
        head.loop_beats = Some((8.0, 24.0));
        p.set_transport(&head);
        assert_eq!(p.context.project_time_music, 14.5);
        assert_eq!(p.context.bar_position_music, 14.0);
        assert_eq!(p.context.project_time_samples, 316_000);
        assert_eq!(p.context.continuous_time_samples, 999);
        assert_eq!(p.context.cycle_start_music, 8.0);
        assert_eq!(p.context.cycle_end_music, 24.0);
        for bit in [super::ctx_state::PLAYING, super::ctx_state::RECORDING,
                    super::ctx_state::CYCLE_ACTIVE, super::ctx_state::CYCLE_VALID,
                    super::ctx_state::PROJECT_TIME_MUSIC_VALID,
                    super::ctx_state::BAR_POSITION_VALID,
                    super::ctx_state::CONT_TIME_VALID] {
            assert_ne!(p.context.state & bit, 0, "bit {bit} was not set");
        }

        // And it renders with the context attached rather than a null one.
        let mut out = vec![0.0f32; 128 * 2];
        p.process(&mut out, 128);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    /// The factory of an installed bundle declares what it is. Cordis
    /// registers Instrument|Piano|Synth; a scan that shows it needs no
    /// second literal for that.
    #[test]
    fn the_scan_reads_the_sub_categories_the_factory_declares() {
        let Some(path) = super::find_by_name("Cordis") else {
            eprintln!("Cordis is not installed; skipping");
            return;
        };
        let info = super::scan_all().into_iter().find(|p| p.path == path).expect("Cordis scanned");
        assert!(info.is_instrument, "{:?}", info.subcategories);
        assert!(!info.is_effect, "{:?}", info.subcategories);
        assert_eq!(info.vendor, "Phonix Audio");
        assert_eq!(info.category(), Some("Piano"), "{:?}", info.subcategories);
    }

    use super::*;

    /// A throwaway bundle tree. No `tempfile` dependency; the pid keeps parallel
    /// test binaries apart and `Drop` cleans up.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("phonix_vst3_{}_{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn path(&self) -> &Path { &self.0 }
        /// Lay out `<name>.vst3/Contents/<arch>/<name>.<ext>`.
        fn bundle(&self, at: &Path, name: &str, arch: &str, ext: &str) -> PathBuf {
            let bundle = at.join(format!("{name}.vst3"));
            let dir = bundle.join("Contents").join(arch);
            std::fs::create_dir_all(&dir).unwrap();
            let file = if ext.is_empty() { dir.join(name) } else { dir.join(format!("{name}.{ext}")) };
            std::fs::write(&file, b"not a real module").unwrap();
            bundle
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    const OTHER_ARCH: &str = if cfg!(target_os = "windows") { "x86_64-linux" } else { "x86_64-win" };
    const OTHER_EXT: &str = if cfg!(target_os = "windows") { "so" } else { "vst3" };

    #[test]
    fn a_bundle_built_for_this_host_resolves() {
        let fx = Fixture::new("ok");
        let bundle = fx.bundle(fx.path(), "Cordis", VST3_ARCH_DIR, VST3_MODULE_EXT);
        let module = resolve_module_path(&bundle).expect("this host's own arch must resolve");
        assert!(module.starts_with(&bundle));
        assert_eq!(module.parent().unwrap().file_name().unwrap(), VST3_ARCH_DIR);
    }

    /// The reason the error is typed. A bundle correctly installed for another
    /// architecture used to report as "not found", which sent everyone hunting
    /// for a missing file instead of reading the one word that mattered.
    #[test]
    fn a_bundle_built_for_another_host_says_which_arch_it_carries() {
        let fx = Fixture::new("arch");
        let bundle = fx.bundle(fx.path(), "Foreign", OTHER_ARCH, OTHER_EXT);
        match resolve_module_path(&bundle) {
            Err(BundleError::WrongArch { wanted, found }) => {
                assert_eq!(wanted, VST3_ARCH_DIR);
                assert_eq!(found, vec![OTHER_ARCH.to_string()]);
            }
            other => panic!("expected WrongArch, got {other:?}"),
        }
    }

    /// Vendors nest their bundles inside the search path, so the flat `read_dir`
    /// missed most of a real installation. A foreign-arch bundle must not be
    /// offered at all.
    #[test]
    fn the_scan_descends_into_vendor_folders_and_skips_foreign_bundles() {
        let fx = Fixture::new("scan");
        let vendor = fx.path().join("SomeVendor").join("Pianos");
        std::fs::create_dir_all(&vendor).unwrap();
        fx.bundle(&vendor, "Nested", VST3_ARCH_DIR, VST3_MODULE_EXT);
        fx.bundle(fx.path(), "Flat", VST3_ARCH_DIR, VST3_MODULE_EXT);
        fx.bundle(fx.path(), "Foreign", OTHER_ARCH, OTHER_EXT);

        let names: Vec<String> = scan_vst3_dir(fx.path()).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Flat", "Nested"]);
    }

    /// A bundle is a leaf. `Contents` is not a vendor folder, and whatever sits
    /// inside it is not a second plugin.
    #[test]
    fn the_scan_does_not_walk_inside_a_bundle() {
        let fx = Fixture::new("inner");
        let outer = fx.bundle(fx.path(), "Outer", VST3_ARCH_DIR, VST3_MODULE_EXT);
        fx.bundle(&outer.join("Contents"), "Inner", VST3_ARCH_DIR, VST3_MODULE_EXT);

        let names: Vec<String> = scan_vst3_dir(fx.path()).into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["Outer"]);
    }

    /// End to end, and the whole point of this work: take a bundle this repo
    /// built for THIS platform, load it through the real loader, play a note and
    /// assert the result is not silence.
    ///
    /// Ignored because it needs `scripts/build_plugins.sh` to have run — or any
    /// bundle on `$PHONIX_VST3_PATH`. Run with:
    ///   cargo test --lib vst3_host::tests::a_bundle_this_repo_built -- --ignored --nocapture
    #[test]
    #[ignore = "needs a bundle built by scripts/build_plugins.sh on PHONIX_VST3_PATH"]
    fn a_bundle_this_repo_built_loads_and_makes_sound() {
        let found = scan_all();
        assert!(
            !found.is_empty(),
            "no VST3 bundle found; searched {:?}",
            default_vst3_dirs()
        );
        let info = found
            .iter()
            .find(|p| p.name == "Cordis")
            .unwrap_or(&found[0]);
        eprintln!("loading {} from {}", info.name, info.path.display());

        let mut plugin = Vst3Plugin::load(&info.path, 48_000.0, 512)
            .unwrap_or_else(|e| panic!("{}: {e}", info.name));

        // The id a session stores to find this plugin again on another machine.
        // Asserted against the literal rather than a hand-computed string, so
        // this also pins the byte ORDER, where VST3 has a platform trap.
        if info.name == "Cordis" {
            let expected =
                String::from_utf8_lossy(&phonix_plugin::vstpreset::class_id_to_hex(b"PxCordisPiano001"))
                    .into_owned();
            assert_eq!(plugin.class_id_hex(), expected, "class id read back wrong");
        }
        eprintln!("class id = {}", plugin.class_id_hex());

        plugin.note_on(60, 100);

        let mut out = vec![0.0f32; 512 * 2];
        let mut peak = 0.0f32;
        for _ in 0..40 {
            plugin.process(&mut out, 512);
            for s in &out { peak = peak.max(s.abs()); }
        }
        eprintln!("peak = {peak:.6}");
        assert!(peak > 1e-4, "{} loaded but rendered silence", info.name);
    }

    /// What the parameter cache reads back from a real plugin.
    ///
    /// The host used to keep id, title and value and nothing else, so it drew a
    /// bare 0..1 slider for everything and left the window's heading blank. All
    /// of this was published all along.
    #[test]
    #[ignore]
    fn the_parameter_cache_carries_what_the_plugin_publishes() {
        let Some(path) = find_by_name("Cordis") else {
            eprintln!("Cordis is not installed; skipping");
            return;
        };
        let plugin = Vst3Plugin::load(&path, 48_000.0, 512).expect("load");
        let cache = plugin.build_param_cache();

        assert!(!cache.plugin_name.is_empty(), "the window heading was blank");
        eprintln!("plugin_name = {:?}, {} visible params", cache.plugin_name, cache.params.len());

        // The 2080 controller parameters are flagged hidden and must not show.
        assert!(
            cache.params.len() < 64,
            "{} parameters: the hidden MIDI-CC range leaked into the grid",
            cache.params.len()
        );

        let gain = cache.params.iter().find(|p| p.title == "Gain").expect("Gain");
        assert!(!gain.display.is_empty(), "no formatted value for Gain");
        eprintln!("Gain shows as {:?} (default {:.3})", gain.display, gain.default);

        let hybrid = cache.params.iter().find(|p| p.title.contains("Hybrid")).expect("Hybrid");
        assert!(hybrid.is_toggle(), "a boolean must read as a toggle, not a slider");

        let unison = cache.params.iter().find(|p| p.title == "Unison").expect("Unison");
        assert_eq!(unison.units.trim(), "cents", "the unit was dropped");
    }

    /// The pedal has to actually arrive, and VST3 carries no controller event.
    /// This exercises the whole path at once: `IMidiMapping` read at load, the
    /// parameter queue, and the plugin's own `MidiCCs` gate.
    #[test]
    #[ignore]
    fn a_hosted_piano_gets_its_sustain_pedal() {
        let Some(path) = find_by_name("Cordis") else {
            eprintln!("Cordis is not installed; skipping");
            return;
        };

        // The same note, released at the same moment, with and without a pedal.
        let tail_after_release = |pedal: bool| -> f32 {
            let mut p = Vst3Plugin::load(&path, 48_000.0, 512).expect("load");
            assert!(p.maps_cc(64), "the plugin publishes no mapping for CC64");
            if pedal {
                assert!(p.send_cc(64, 127, 0), "CC64 refused");
            }
            let mut out = vec![0.0f32; 512 * 2];
            p.note_on(60, 100);
            for _ in 0..20 { p.process(&mut out, 512); }
            p.note_off(60);
            for _ in 0..40 { p.process(&mut out, 512); }
            let mut peak = 0.0f32;
            for _ in 0..20 {
                p.process(&mut out, 512);
                for s in &out { peak = peak.max(s.abs()); }
            }
            peak
        };

        let with = tail_after_release(true);
        let without = tail_after_release(false);
        eprintln!("tail with pedal {with:.6}, without {without:.6}");
        assert!(
            with > without * 4.0,
            "the pedal changed nothing: {with:.6} with, {without:.6} without"
        );
    }

    /// A chain pushed on the FX page and saved with the project, then a
    /// preset change from the host: the chain must be the preset's again,
    /// not the pushed one with the preset's picture over it.
    #[test]
    #[ignore]
    fn a_preset_change_puts_the_edited_chain_back() {
        let Some(path) = find_by_name("Cordis") else {
            eprintln!("Cordis is not installed; skipping");
            return;
        };
        let preset_id = Vst3Plugin::load(&path, 48_000.0, 512)
            .expect("load")
            .build_param_cache()
            .params
            .iter()
            .find(|p| p.title == "Preset")
            .expect("a Preset parameter")
            .id;
        // The parameter counts 0 (Init) then the bank; the second preset.
        let second = 2.0 / 10.0;
        let peak_of = |p: &mut Vst3Plugin| -> f32 {
            let mut out = vec![0.0f32; 512 * 2];
            for _ in 0..4 { p.process(&mut out, 512); }
            p.note_on(48, 120);
            let mut peak = 0.0f32;
            for _ in 0..60 {
                p.process(&mut out, 512);
                for s in &out { peak = peak.max(s.abs()); }
            }
            p.note_off(48);
            for _ in 0..80 { p.process(&mut out, 512); }
            peak
        };

        // What the second preset sounds like from a fresh instance.
        let reference = {
            let mut p = Vst3Plugin::load(&path, 48_000.0, 512).expect("load");
            p.set_param(preset_id, second);
            peak_of(&mut p)
        };

        // A saved project whose compressor was pushed until it saturates.
        let mut p = Vst3Plugin::load(&path, 48_000.0, 512).expect("load");
        p.set_param(preset_id, 1.0 / 10.0);
        let mut out = vec![0.0f32; 512 * 2];
        for _ in 0..4 { p.process(&mut out, 512); }
        let mut state: serde_json::Value = serde_json::from_slice(&p.get_state()).expect("state is JSON");
        let mut patch: serde_json::Value = serde_json::from_str(state["fields"]["patch"].as_str().expect("the patch field")).expect("the patch is JSON");
        patch["fx"]["slots"][1]["params"]["threshold"] = serde_json::json!(-60.0);
        patch["fx"]["slots"][1]["params"]["makeup"] = serde_json::json!(24.0);
        state["fields"]["patch"] = serde_json::Value::String(patch.to_string());
        p.set_state(state.to_string().as_bytes());
        let pushed = peak_of(&mut p);

        // The host moves to the second preset.
        p.set_param(preset_id, second);
        let after = peak_of(&mut p);

        eprintln!("reference {reference:.4}, pushed {pushed:.4}, after the preset change {after:.4}");
        assert!(pushed > reference * 2.0, "the pushed compressor did not saturate: {pushed:.4} vs {reference:.4}");
        assert!((after - reference).abs() < reference * 0.05, "the chain kept the edit: {after:.4} vs {reference:.4}");
    }

    /// Only directories that exist are offered, so a caller can iterate the list
    /// without stat-ing each entry itself.
    #[test]
    fn the_default_search_paths_all_exist() {
        for d in default_vst3_dirs() {
            assert!(d.is_dir(), "{} is listed but is not a directory", d.display());
        }
    }
}
