//! CLAP plugin hosting: the plugins found on disk, one loaded in a
//! process, notes and parameters in, audio out, state and latency.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::*;
use clap_sys::ext::audio_ports::{clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS, clap_audio_port_info};
use clap_sys::ext::gui::{clap_host_gui, clap_plugin_gui, CLAP_EXT_GUI};
use clap_sys::ext::latency::{clap_host_latency, clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::log::{clap_host_log, clap_log_severity, CLAP_EXT_LOG};
use clap_sys::ext::note_ports::{clap_plugin_note_ports, clap_note_port_info, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI};
use clap_sys::ext::params::{clap_host_params, clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_HIDDEN, CLAP_PARAM_IS_STEPPED};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::ext::thread_check::{clap_host_thread_check, CLAP_EXT_THREAD_CHECK};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::clap_process;
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;
use libloading::{Library, Symbol};

/// Beats to CLAP's fixed point.
const BEATTIME_FACTOR: f64 = (1i64 << 31) as f64;

/// A plugin a `.clap` file offers.
#[derive(Debug, Clone, PartialEq)]
pub struct ClapPluginInfo {
    pub path: String,
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// The plugin's own one-line description. CLAP publishes it in the
    /// descriptor; a browser has no business writing its own.
    pub description: String,
    /// Everything the descriptor's `features` array declares, verbatim:
    /// `instrument`, `synthesizer`, `drum-machine`, `physical-modeling`,
    /// `stereo`... The structural terms and the musical ones arrive
    /// together and are told apart where they are read.
    pub features: Vec<String>,
    pub is_instrument: bool,
    pub is_effect: bool,
}

fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() { return String::new(); }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// The folders CLAP plugins live in: `CLAP_PATH`, the user's, the system's.
pub fn clap_search_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("CLAP_PATH") {
        for d in p.split(':') { if !d.is_empty() { out.push(PathBuf::from(d)); } }
    }
    if let Ok(home) = std::env::var("HOME") {
        out.push(PathBuf::from(home).join(".clap"));
    }
    out.push(PathBuf::from("/usr/lib/clap"));
    out.push(PathBuf::from("/usr/local/lib/clap"));
    out
}

/// Every plugin in every `.clap` under the search paths.
pub fn scan_clap() -> Vec<ClapPluginInfo> {
    let mut out = Vec::new();
    for dir in clap_search_paths() {
        scan_dir(&dir, 0, &mut out);
    }
    out
}

fn scan_dir(dir: &Path, depth: usize, out: &mut Vec<ClapPluginInfo>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 3 { scan_dir(&p, depth + 1, out); }
        } else if p.extension().and_then(|x| x.to_str()) == Some("clap") {
            out.extend(scan_clap_file(&p).unwrap_or_default());
        }
    }
}

/// The plugins one `.clap` file offers.
pub fn scan_clap_file(path: &Path) -> Result<Vec<ClapPluginInfo>, String> {
    let module = Module::open(path)?;
    let mut out = Vec::new();
    unsafe {
        let count = (*module.factory).get_plugin_count.map(|f| f(module.factory)).unwrap_or(0);
        for i in 0..count {
            let Some(get) = (*module.factory).get_plugin_descriptor else { break };
            let d = get(module.factory, i);
            if d.is_null() { continue; }
            out.push(describe(&*d, path));
        }
    }
    Ok(out)
}

fn describe(d: &clap_plugin_descriptor, path: &Path) -> ClapPluginInfo {
    let mut features: Vec<String> = Vec::new();
    if !d.features.is_null() {
        let mut f = d.features;
        unsafe {
            while !(*f).is_null() {
                features.push(cstr_to_string(*f));
                f = f.add(1);
            }
        }
    }
    ClapPluginInfo {
        path: path.to_string_lossy().into_owned(),
        id: cstr_to_string(d.id),
        name: cstr_to_string(d.name),
        vendor: cstr_to_string(d.vendor),
        description: cstr_to_string(d.description),
        is_instrument: features.iter().any(|f| f == "instrument"),
        is_effect: features.iter().any(|f| f == "audio-effect"),
        features,
    }
}

/// A `.clap` file opened, its entry initialised, its factory at hand.
struct Module {
    factory: *const clap_plugin_factory,
    entry: *const clap_plugin_entry,
    _lib: Library,
}

impl Module {
    fn open(path: &Path) -> Result<Self, String> {
        let lib = unsafe { Library::new(path) }.map_err(|e| format!("{}: {e}", path.display()))?;
        let entry: *const clap_plugin_entry = unsafe {
            let sym: Symbol<*const clap_plugin_entry> = lib.get(b"clap_entry\0")
                .map_err(|e| format!("{}: no clap_entry: {e}", path.display()))?;
            *sym
        };
        if entry.is_null() { return Err("clap_entry is null".into()); }
        let cpath = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let ok = unsafe { (*entry).init.map(|f| f(cpath.as_ptr())).unwrap_or(true) };
        if !ok { return Err(format!("{}: clap_entry.init refused", path.display())); }
        let factory = unsafe {
            (*entry).get_factory.map(|f| f(CLAP_PLUGIN_FACTORY_ID.as_ptr())).unwrap_or(std::ptr::null())
        } as *const clap_plugin_factory;
        if factory.is_null() {
            unsafe { if let Some(d) = (*entry).deinit { d(); } }
            return Err(format!("{}: no plugin factory", path.display()));
        }
        Ok(Self { factory, entry, _lib: lib })
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        unsafe { if let Some(d) = (*self.entry).deinit { d(); } }
    }
}

// The factory and the entry are read while loading and while an editor is
// taken apart, never written. The `Arc` exists to keep the code mapped for as
// long as anything points into it.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

/// What the plugin asked the host for since the last look.
struct HostData {
    restart: AtomicBool,
    params_rescan: AtomicBool,
    latency_changed: AtomicBool,
    name: CString,
    vendor: CString,
    url: CString,
    version: CString,
}

/// The host callbacks: what we answer to, and the requests we note.
struct HostBlock {
    host: clap_host,
    data: HostData,
    params: clap_host_params,
    latency: clap_host_latency,
    log: clap_host_log,
    thread_check: clap_host_thread_check,
    gui: clap_host_gui,
}

unsafe fn host_data<'a>(host: *const clap_host) -> &'a HostBlock {
    &*((*host).host_data as *const HostBlock)
}

unsafe extern "C" fn host_get_extension(host: *const clap_host, id: *const c_char) -> *const c_void {
    let h = host_data(host);
    let id = CStr::from_ptr(id);
    if id == CLAP_EXT_PARAMS { return &h.params as *const _ as *const c_void; }
    if id == CLAP_EXT_LATENCY { return &h.latency as *const _ as *const c_void; }
    if id == CLAP_EXT_LOG { return &h.log as *const _ as *const c_void; }
    if id == CLAP_EXT_THREAD_CHECK { return &h.thread_check as *const _ as *const c_void; }
    if id == CLAP_EXT_GUI { return &h.gui as *const _ as *const c_void; }
    std::ptr::null()
}
unsafe extern "C" fn host_gui_resize_hints_changed(_host: *const clap_host) {}
unsafe extern "C" fn host_gui_request_resize(_host: *const clap_host, _w: u32, _h: u32) -> bool { false }
unsafe extern "C" fn host_gui_request_show(_host: *const clap_host) -> bool { false }
unsafe extern "C" fn host_gui_request_hide(_host: *const clap_host) -> bool { false }
unsafe extern "C" fn host_gui_closed(_host: *const clap_host, _was_destroyed: bool) {}
unsafe extern "C" fn host_request_restart(host: *const clap_host) { host_data(host).data.restart.store(true, Ordering::Relaxed); }
unsafe extern "C" fn host_request_process(_host: *const clap_host) {}
unsafe extern "C" fn host_request_callback(_host: *const clap_host) {}
unsafe extern "C" fn host_params_rescan(host: *const clap_host, _flags: u32) { host_data(host).data.params_rescan.store(true, Ordering::Relaxed); }
unsafe extern "C" fn host_params_clear(_host: *const clap_host, _id: clap_id, _flags: u32) {}
unsafe extern "C" fn host_params_request_flush(_host: *const clap_host) {}
unsafe extern "C" fn host_latency_changed(host: *const clap_host) { host_data(host).data.latency_changed.store(true, Ordering::Relaxed); }
unsafe extern "C" fn host_log(_host: *const clap_host, severity: clap_log_severity, msg: *const c_char) {
    let m = cstr_to_string(msg);
    if severity >= 2 { log::warn!("clap: {m}"); } else { log::info!("clap: {m}"); }
}
unsafe extern "C" fn host_is_main_thread(_host: *const clap_host) -> bool { true }
unsafe extern "C" fn host_is_audio_thread(_host: *const clap_host) -> bool { true }

impl HostBlock {
    fn new() -> Box<Self> {
        let data = HostData {
            restart: AtomicBool::new(false),
            params_rescan: AtomicBool::new(false),
            latency_changed: AtomicBool::new(false),
            name: CString::new("Phonix").unwrap(),
            vendor: CString::new("Phonix Audio").unwrap(),
            url: CString::new("https://github.com/phonix-audio").unwrap(),
            version: CString::new(env!("CARGO_PKG_VERSION")).unwrap(),
        };
        let mut b = Box::new(HostBlock {
            host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: data.name.as_ptr(),
                vendor: data.vendor.as_ptr(),
                url: data.url.as_ptr(),
                version: data.version.as_ptr(),
                get_extension: Some(host_get_extension),
                request_restart: Some(host_request_restart),
                request_process: Some(host_request_process),
                request_callback: Some(host_request_callback),
            },
            data,
            params: clap_host_params { rescan: Some(host_params_rescan), clear: Some(host_params_clear), request_flush: Some(host_params_request_flush) },
            latency: clap_host_latency { changed: Some(host_latency_changed) },
            log: clap_host_log { log: Some(host_log) },
            thread_check: clap_host_thread_check { is_main_thread: Some(host_is_main_thread), is_audio_thread: Some(host_is_audio_thread) },
            gui: clap_host_gui {
                resize_hints_changed: Some(host_gui_resize_hints_changed),
                request_resize: Some(host_gui_request_resize),
                request_show: Some(host_gui_request_show),
                request_hide: Some(host_gui_request_hide),
                closed: Some(host_gui_closed),
            },
        });
        let p: *mut HostBlock = &mut *b;
        b.host.host_data = p as *mut c_void;
        b
    }
}

/// One event of the block, whatever its kind.
#[repr(C)]
#[derive(Clone, Copy)]
union AnyEvent {
    header: clap_event_header,
    note: clap_event_note,
    param: clap_event_param_value,
    midi: clap_event_midi,
    expr: clap_event_note_expression,
}

/// The events handed to the plugin this block.
struct InEvents {
    list: clap_input_events,
    events: Vec<AnyEvent>,
}

unsafe extern "C" fn in_events_size(list: *const clap_input_events) -> u32 {
    let ev = &*((*list).ctx as *const InEvents);
    ev.events.len() as u32
}
unsafe extern "C" fn in_events_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    let ev = &*((*list).ctx as *const InEvents);
    match ev.events.get(index as usize) {
        Some(e) => &e.header as *const clap_event_header,
        None => std::ptr::null(),
    }
}
/// Parameter values the plugin sent back this block are noted so the
/// interface can read them again.
struct OutEvents {
    list: clap_output_events,
    param_changed: AtomicBool,
}
unsafe extern "C" fn out_events_try_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
    if event.is_null() { return false; }
    let out = &*((*list).ctx as *const OutEvents);
    if (*event).space_id == CLAP_CORE_EVENT_SPACE_ID && matches!((*event).type_, CLAP_EVENT_PARAM_VALUE | CLAP_EVENT_PARAM_GESTURE_END) {
        out.param_changed.store(true, Ordering::Relaxed);
    }
    true
}

unsafe extern "C" fn ostream_write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    let v = &mut *((*stream).ctx as *mut Vec<u8>);
    let bytes = std::slice::from_raw_parts(buffer as *const u8, size as usize);
    v.extend_from_slice(bytes);
    size as i64
}
struct ReadCursor<'a> { data: &'a [u8], pos: usize }
unsafe extern "C" fn istream_read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    let c = &mut *((*stream).ctx as *mut ReadCursor);
    let n = (size as usize).min(c.data.len() - c.pos);
    std::ptr::copy_nonoverlapping(c.data.as_ptr().add(c.pos), buffer as *mut u8, n);
    c.pos += n;
    n as i64
}

/// What a window needs to show a plugin's own editor.
///
/// The two pointers belong to a plugin instance the audio thread owns, so
/// this carries what makes using them safe: a flag the instance clears when
/// it is destroyed, and a reference to the module, which keeps the code
/// mapped while the window is taken apart. Without the flag, a host that
/// rebuilt its engines while an editor was open called `destroy` on freed
/// memory.
#[derive(Clone)]
pub struct ClapEditorConn {
    pub(crate) plugin: *const clap_plugin,
    pub(crate) gui: *const clap_plugin_gui,
    alive: Arc<AtomicBool>,
    /// Keeps the shared library mapped. Never read; dropping the last one
    /// unloads the code the editor runs.
    _module: Arc<Module>,
}
unsafe impl Send for ClapEditorConn {}

impl ClapEditorConn {
    /// Whether the plugin instance these pointers name is still there.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

/// A parameter the plugin exposes, for a control.
#[derive(Debug, Clone, PartialEq)]
pub struct ClapParamInfo {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub stepped: bool,
}

impl ClapParamInfo {
    /// A value from 0 to 1 over the parameter's range.
    pub fn normalize(&self, v: f64) -> f32 {
        if self.max <= self.min { return 0.0; }
        ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0) as f32
    }
    pub fn denormalize(&self, n: f32) -> f64 {
        self.min + (self.max - self.min) * n.clamp(0.0, 1.0) as f64
    }
}

/// Channel-voice status bytes, channel 1. One channel is all a host with one
/// instrument per plugin has to say.
const CONTROL_CHANGE: u8 = 0xB0;
const CHANNEL_PRESSURE: u8 = 0xD0;
const PITCH_BEND: u8 = 0xE0;

/// The pitch wheel's 14 bits, as the two data bytes MIDI carries them in:
/// the low seven first, the high seven second.
fn bend_bytes(value: u16) -> [u8; 2] {
    let v = value.min(16_383);
    [(v & 0x7F) as u8, (v >> 7) as u8]
}

/// A loaded plugin: created, activated, processing.
pub struct ClapPlugin {
    plugin: *const clap_plugin,
    host: Box<HostBlock>,
    ext_params: *const clap_plugin_params,
    ext_state: *const clap_plugin_state,
    ext_latency: *const clap_plugin_latency,
    in_events: Box<InEvents>,
    out_events: Box<OutEvents>,
    transport: clap_event_transport,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    in_ptrs: [*mut f32; 2],
    out_ptrs: [*mut f32; 2],
    has_audio_input: bool,
    out_channels: u32,
    midi_dialect: bool,
    /// Whether the note port takes MIDI at all. The CLAP dialect carries
    /// notes and note expressions and nothing else a keyboard sends, so this
    /// is the only way a controller, the wheel or the pedal reaches a plugin.
    accepts_midi: bool,
    held: Vec<u8>,
    steady_time: i64,
    sr: f32,
    block: usize,
    active: bool,
    processing: bool,
    params: Vec<ClapParamInfo>,
    pub info: ClapPluginInfo,
    /// Cleared before the instance is destroyed. An editor window holds raw
    /// pointers into the instance and reads this before using them.
    alive: Arc<AtomicBool>,
    // Declared last: the code stays mapped while the plugin is destroyed.
    module: Arc<Module>,
}

unsafe impl Send for ClapPlugin {}

impl ClapPlugin {
    /// Load the plugin `id` (the first one when None) from a `.clap`,
    /// activate it at `sr` for blocks up to `block` frames, and start it.
    pub fn load(path: &Path, id: Option<&str>, sr: f32, block: usize) -> Result<Self, String> {
        let module = Module::open(path)?;
        let host = HostBlock::new();
        let (plugin, info) = unsafe {
            let count = (*module.factory).get_plugin_count.map(|f| f(module.factory)).unwrap_or(0);
            let mut chosen: Option<*const clap_plugin_descriptor> = None;
            for i in 0..count {
                let d = (*module.factory).get_plugin_descriptor.map(|f| f(module.factory, i)).unwrap_or(std::ptr::null());
                if d.is_null() { continue; }
                match id {
                    Some(want) if cstr_to_string((*d).id) != want => continue,
                    _ => { chosen = Some(d); break; }
                }
            }
            let d = chosen.ok_or_else(|| format!("{}: plugin {} not found", path.display(), id.unwrap_or("")))?;
            let create = (*module.factory).create_plugin.ok_or("factory has no create_plugin")?;
            let p = create(module.factory, &host.host, (*d).id);
            if p.is_null() { return Err(format!("{}: create_plugin failed", path.display())); }
            if !(*p).init.map(|f| f(p)).unwrap_or(false) {
                if let Some(des) = (*p).destroy { des(p); }
                return Err(format!("{}: plugin init failed", path.display()));
            }
            (p, describe(&*d, path))
        };
        let get_ext = |id: &CStr| -> *const c_void {
            unsafe { (*plugin).get_extension.map(|f| f(plugin, id.as_ptr())).unwrap_or(std::ptr::null()) }
        };
        let ext_params = get_ext(CLAP_EXT_PARAMS) as *const clap_plugin_params;
        let ext_state = get_ext(CLAP_EXT_STATE) as *const clap_plugin_state;
        let ext_latency = get_ext(CLAP_EXT_LATENCY) as *const clap_plugin_latency;
        let ports = get_ext(CLAP_EXT_AUDIO_PORTS) as *const clap_plugin_audio_ports;
        let (has_audio_input, out_channels) = unsafe {
            if ports.is_null() { (false, 2) } else {
                let ins = (*ports).count.map(|f| f(plugin, true)).unwrap_or(0);
                let outs = (*ports).count.map(|f| f(plugin, false)).unwrap_or(0);
                let mut ch = 2u32;
                if outs > 0 {
                    let mut pi: clap_audio_port_info = std::mem::zeroed();
                    if (*ports).get.map(|f| f(plugin, 0, false, &mut pi)).unwrap_or(false) { ch = pi.channel_count.max(1); }
                }
                (ins > 0, ch)
            }
        };
        let note_ports = get_ext(CLAP_EXT_NOTE_PORTS) as *const clap_plugin_note_ports;
        // What the first note port takes. A plugin that does not speak the
        // CLAP dialect is sent its notes as MIDI; one that takes MIDI at all
        // can also be sent what only MIDI carries.
        let dialects = unsafe {
            if note_ports.is_null() { 0 } else {
                let mut pi: clap_note_port_info = std::mem::zeroed();
                let ok = (*note_ports).count.map(|f| f(plugin, true)).unwrap_or(0) > 0
                    && (*note_ports).get.map(|f| f(plugin, 0, true, &mut pi)).unwrap_or(false);
                if ok { pi.supported_dialects } else { 0 }
            }
        };
        let midi_dialect = dialects != 0 && dialects & CLAP_NOTE_DIALECT_CLAP == 0;
        let accepts_midi = dialects & CLAP_NOTE_DIALECT_MIDI != 0;
        let mut in_events = Box::new(InEvents {
            list: clap_input_events { ctx: std::ptr::null_mut(), size: Some(in_events_size), get: Some(in_events_get) },
            events: Vec::with_capacity(256),
        });
        let ie: *mut InEvents = &mut *in_events;
        in_events.list.ctx = ie as *mut c_void;
        let mut out_events = Box::new(OutEvents {
            list: clap_output_events { ctx: std::ptr::null_mut(), try_push: Some(out_events_try_push) },
            param_changed: AtomicBool::new(false),
        });
        let oe: *mut OutEvents = &mut *out_events;
        out_events.list.ctx = oe as *mut c_void;
        let transport = clap_event_transport {
            header: clap_event_header { size: std::mem::size_of::<clap_event_transport>() as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_TRANSPORT, flags: 0 },
            flags: CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE | CLAP_TRANSPORT_HAS_TIME_SIGNATURE,
            song_pos_beats: 0, song_pos_seconds: 0, tempo: 120.0, tempo_inc: 0.0,
            loop_start_beats: 0, loop_end_beats: 0, loop_start_seconds: 0, loop_end_seconds: 0,
            bar_start: 0, bar_number: 0, tsig_num: 4, tsig_denom: 4,
        };
        let mut me = Self {
            plugin, host, ext_params, ext_state, ext_latency, in_events, out_events, transport,
            in_l: vec![0.0; block], in_r: vec![0.0; block], out_l: vec![0.0; block], out_r: vec![0.0; block],
            in_ptrs: [std::ptr::null_mut(); 2], out_ptrs: [std::ptr::null_mut(); 2],
            has_audio_input, out_channels, midi_dialect, accepts_midi, held: Vec::new(), steady_time: 0,
            sr, block, active: false, processing: false, params: Vec::new(), info,
            alive: Arc::new(AtomicBool::new(true)), module: Arc::new(module),
        };
        me.refresh_params();
        me.activate()?;
        Ok(me)
    }

    fn activate(&mut self) -> Result<(), String> {
        unsafe {
            let ok = (*self.plugin).activate.map(|f| f(self.plugin, self.sr as f64, 1, self.block as u32)).unwrap_or(false);
            if !ok { return Err(format!("{}: activate failed", self.info.name)); }
            self.active = true;
            self.processing = (*self.plugin).start_processing.map(|f| f(self.plugin)).unwrap_or(true);
        }
        Ok(())
    }

    fn deactivate(&mut self) {
        unsafe {
            if self.processing { if let Some(f) = (*self.plugin).stop_processing { f(self.plugin); } }
            if self.active { if let Some(f) = (*self.plugin).deactivate { f(self.plugin); } }
        }
        self.processing = false;
        self.active = false;
    }

    /// Run again at another rate or block size.
    pub fn set_sample_rate(&mut self, sr: f32, block: usize) {
        self.deactivate();
        self.sr = sr;
        self.block = block;
        for v in [&mut self.in_l, &mut self.in_r, &mut self.out_l, &mut self.out_r] { v.resize(block, 0.0); }
        let _ = self.activate();
    }

    pub fn has_audio_input(&self) -> bool { self.has_audio_input }

    /// The plugin's editor, when it has one.
    pub fn editor_conn(&self) -> Option<ClapEditorConn> {
        let gui = unsafe { (*self.plugin).get_extension.map(|f| f(self.plugin, CLAP_EXT_GUI.as_ptr())).unwrap_or(std::ptr::null()) } as *const clap_plugin_gui;
        (!gui.is_null()).then_some(ClapEditorConn {
            plugin: self.plugin,
            gui,
            alive: self.alive.clone(),
            _module: self.module.clone(),
        })
    }

    /// Where the transport stands for the next block.
    pub fn set_transport(&mut self, head: &crate::plugin::PlayHead) {
        self.transport.tempo = head.tempo;
        self.transport.song_pos_beats = (head.beats * BEATTIME_FACTOR) as i64;
        self.transport.bar_start = (head.bar_start_beats * BEATTIME_FACTOR) as i64;
        self.transport.bar_number = 0;
        self.transport.tsig_num = head.time_sig.0 as u16;
        self.transport.tsig_denom = head.time_sig.1.max(1) as u16;
        let mut flags = CLAP_TRANSPORT_HAS_TEMPO
            | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
            | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
        if head.playing { flags |= CLAP_TRANSPORT_IS_PLAYING; }
        if head.recording { flags |= CLAP_TRANSPORT_IS_RECORDING; }
        if let Some((start, end)) = head.loop_beats {
            self.transport.loop_start_beats = (start * BEATTIME_FACTOR) as i64;
            self.transport.loop_end_beats = (end * BEATTIME_FACTOR) as i64;
            flags |= CLAP_TRANSPORT_IS_LOOP_ACTIVE;
        }
        self.transport.flags = flags;
    }

    fn push_event(&mut self, ev: AnyEvent) {
        if self.in_events.events.len() < 4096 { self.in_events.events.push(ev); }
    }

    fn note_event(&mut self, on: bool, key: u8, velocity: u8) {
        if self.midi_dialect {
            let status = if on { 0x90 } else { 0x80 };
            let e = clap_event_midi {
                header: clap_event_header { size: std::mem::size_of::<clap_event_midi>() as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_MIDI, flags: 0 },
                port_index: 0,
                data: [status, key & 0x7F, velocity & 0x7F],
            };
            self.push_event(AnyEvent { midi: e });
        } else {
            let e = clap_event_note {
                header: clap_event_header { size: std::mem::size_of::<clap_event_note>() as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: if on { CLAP_EVENT_NOTE_ON } else { CLAP_EVENT_NOTE_OFF }, flags: 0 },
                note_id: -1, port_index: 0, channel: 0, key: key as i16,
                velocity: velocity as f64 / 127.0,
            };
            self.push_event(AnyEvent { note: e });
        }
    }

    pub fn note_on(&mut self, key: u8, velocity: u8) {
        if !self.held.contains(&key) { self.held.push(key); }
        self.note_event(true, key, velocity.max(1));
    }

    pub fn note_off(&mut self, key: u8) {
        self.held.retain(|&k| k != key);
        self.note_event(false, key, 0);
    }

    pub fn all_notes_off(&mut self) {
        let held = std::mem::take(&mut self.held);
        for k in held { self.note_event(false, k, 0); }
    }

    /// Queue a note expression on every sounding instance of `key`, in
    /// the expression's own units.
    pub fn note_expression(&mut self, expression_id: i32, key: u8, value: f64) {
        if self.midi_dialect { return; }
        let e = clap_event_note_expression {
            header: clap_event_header { size: std::mem::size_of::<clap_event_note_expression>() as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_NOTE_EXPRESSION, flags: 0 },
            expression_id, note_id: -1, port_index: 0, channel: -1, key: key as i16, value,
        };
        self.push_event(AnyEvent { expr: e });
    }

    /// Queue one channel-voice MIDI message.
    ///
    /// Returns false when the plugin's note port does not take MIDI, which is
    /// not an error: the CLAP dialect has no way to carry a controller, and a
    /// plugin that speaks only it is answering everything it can be sent.
    pub fn send_midi(&mut self, data: [u8; 3]) -> bool {
        if !self.accepts_midi { return false; }
        let e = clap_event_midi {
            header: clap_event_header { size: std::mem::size_of::<clap_event_midi>() as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_MIDI, flags: 0 },
            port_index: 0,
            data,
        };
        self.push_event(AnyEvent { midi: e });
        true
    }

    /// A control change: the pedal, the wheel, whatever the keyboard sends.
    pub fn send_cc(&mut self, cc: u8, value: u8) -> bool {
        self.send_midi([CONTROL_CHANGE, cc & 0x7F, value & 0x7F])
    }

    /// The pitch wheel, 14 bits, 8192 at rest.
    pub fn pitch_bend(&mut self, value: u16) -> bool {
        let [lo, hi] = bend_bytes(value);
        self.send_midi([PITCH_BEND, lo, hi])
    }

    /// Channel pressure, the aftertouch a keyboard sends for the whole
    /// channel rather than per key.
    pub fn channel_pressure(&mut self, value: u8) -> bool {
        self.send_midi([CHANNEL_PRESSURE, value & 0x7F, 0])
    }

    /// Queue a parameter value, in the parameter's own range.
    pub fn set_param(&mut self, id: u32, value: f64) {
        let e = clap_event_param_value {
            header: clap_event_header { size: std::mem::size_of::<clap_event_param_value>() as u32, time: 0, space_id: CLAP_CORE_EVENT_SPACE_ID, type_: CLAP_EVENT_PARAM_VALUE, flags: 0 },
            param_id: id, cookie: std::ptr::null_mut(), note_id: -1, port_index: -1, channel: -1, key: -1, value,
        };
        self.push_event(AnyEvent { param: e });
    }

    /// Queue a parameter value given from 0 to 1.
    pub fn set_param_normalized(&mut self, id: u32, n: f32) {
        if let Some(p) = self.params.iter().find(|p| p.id == id) {
            let v = p.denormalize(n);
            self.set_param(id, v);
        }
    }

    /// The parameter's current value, in its own range.
    pub fn get_param(&self, id: u32) -> Option<f64> {
        if self.ext_params.is_null() { return None; }
        let mut v = 0.0f64;
        let ok = unsafe { (*self.ext_params).get_value.map(|f| f(self.plugin, id, &mut v)).unwrap_or(false) };
        ok.then_some(v)
    }

    /// The parameter's value as the plugin prints it.
    pub fn param_text(&self, id: u32, value: f64) -> Option<String> {
        if self.ext_params.is_null() { return None; }
        let mut buf = [0 as c_char; 128];
        let ok = unsafe { (*self.ext_params).value_to_text.map(|f| f(self.plugin, id, value, buf.as_mut_ptr(), buf.len() as u32)).unwrap_or(false) };
        ok.then(|| cstr_to_string(buf.as_ptr()))
    }

    pub fn params(&self) -> &[ClapParamInfo] { &self.params }

    /// Read the parameter list again, as after the plugin asked for a rescan.
    pub fn refresh_params(&mut self) {
        self.params.clear();
        if self.ext_params.is_null() { return; }
        unsafe {
            let n = (*self.ext_params).count.map(|f| f(self.plugin)).unwrap_or(0);
            for i in 0..n {
                let mut info: clap_param_info = std::mem::zeroed();
                if !(*self.ext_params).get_info.map(|f| f(self.plugin, i, &mut info)).unwrap_or(false) { continue; }
                if info.flags & CLAP_PARAM_IS_HIDDEN != 0 { continue; }
                self.params.push(ClapParamInfo {
                    id: info.id,
                    name: cstr_to_string(info.name.as_ptr()),
                    min: info.min_value,
                    max: info.max_value,
                    default: info.default_value,
                    stepped: info.flags & CLAP_PARAM_IS_STEPPED != 0,
                });
            }
        }
    }

    /// Whether the plugin asked for its parameters to be read again.
    pub fn take_params_rescan(&self) -> bool { self.host.data.params_rescan.swap(false, Ordering::Relaxed) }
    /// Whether the plugin changed a parameter itself, from its window.
    pub fn take_param_changed(&self) -> bool { self.out_events.param_changed.swap(false, Ordering::Relaxed) }
    /// Whether the plugin asked to be restarted.
    pub fn take_restart(&self) -> bool { self.host.data.restart.swap(false, Ordering::Relaxed) }

    pub fn latency_samples(&self) -> usize {
        if self.ext_latency.is_null() { return 0; }
        unsafe { (*self.ext_latency).get.map(|f| f(self.plugin) as usize).unwrap_or(0) }
    }

    pub fn save_state(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        if self.ext_state.is_null() { return out; }
        let stream = clap_ostream { ctx: &mut out as *mut Vec<u8> as *mut c_void, write: Some(ostream_write) };
        unsafe { (*self.ext_state).save.map(|f| f(self.plugin, &stream)); }
        out
    }

    pub fn load_state(&mut self, data: &[u8]) -> bool {
        if self.ext_state.is_null() || data.is_empty() { return false; }
        let mut cur = ReadCursor { data, pos: 0 };
        let stream = clap_istream { ctx: &mut cur as *mut ReadCursor as *mut c_void, read: Some(istream_read) };
        let ok = unsafe { (*self.ext_state).load.map(|f| f(self.plugin, &stream)).unwrap_or(false) };
        if ok { self.refresh_params(); }
        ok
    }

    /// Run one block: `input` interleaved stereo or None for an
    /// instrument, `output` interleaved stereo written in full.
    pub fn process(&mut self, input: Option<&[f32]>, output: &mut [f32], frames: usize) {
        let frames = frames.min(self.block);
        if !self.processing { for s in output.iter_mut() { *s = 0.0; } self.in_events.events.clear(); return; }
        for i in 0..frames {
            let (l, r) = match input { Some(inp) => (inp[i * 2], inp[i * 2 + 1]), None => (0.0, 0.0) };
            self.in_l[i] = l;
            self.in_r[i] = r;
            self.out_l[i] = 0.0;
            self.out_r[i] = 0.0;
        }
        self.in_ptrs = [self.in_l.as_mut_ptr(), self.in_r.as_mut_ptr()];
        self.out_ptrs = [self.out_l.as_mut_ptr(), self.out_r.as_mut_ptr()];
        let in_buf = clap_audio_buffer { data32: self.in_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: 2, latency: 0, constant_mask: 0 };
        let mut out_buf = clap_audio_buffer { data32: self.out_ptrs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: self.out_channels.min(2), latency: 0, constant_mask: 0 };
        let process = clap_process {
            steady_time: self.steady_time,
            frames_count: frames as u32,
            transport: &self.transport,
            audio_inputs: if self.has_audio_input { &in_buf } else { std::ptr::null() },
            audio_outputs: &mut out_buf,
            audio_inputs_count: if self.has_audio_input { 1 } else { 0 },
            audio_outputs_count: 1,
            in_events: &self.in_events.list,
            out_events: &self.out_events.list,
        };
        unsafe { (*self.plugin).process.map(|f| f(self.plugin, &process)); }
        self.in_events.events.clear();
        self.steady_time += frames as i64;
        let mono = self.out_channels == 1;
        for i in 0..frames {
            output[i * 2] = self.out_l[i];
            output[i * 2 + 1] = if mono { self.out_l[i] } else { self.out_r[i] };
        }
        for s in output.iter_mut().skip(frames * 2) { *s = 0.0; }
    }
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        // Before anything else: an editor window may still hold pointers into
        // this instance, on another thread.
        self.alive.store(false, Ordering::Release);
        self.deactivate();
        unsafe { if let Some(f) = (*self.plugin).destroy { f(self.plugin); } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pitch wheel's two data bytes, as MIDI orders them: low seven bits
    /// first. Swapping them puts a wheel at rest a whole range out.
    #[test]
    fn the_wheel_is_split_low_seven_bits_first() {
        assert_eq!(bend_bytes(8192), [0, 64], "the wheel at rest");
        assert_eq!(bend_bytes(0), [0, 0]);
        assert_eq!(bend_bytes(16_383), [127, 127]);
        assert_eq!(bend_bytes(u16::MAX), [127, 127], "past the range, at the top");
        // Every byte a plugin is handed is a data byte.
        for v in (0..=16_383u16).step_by(97) {
            let [lo, hi] = bend_bytes(v);
            assert!(lo < 128 && hi < 128, "{v} made a status byte");
            assert_eq!(lo as u16 | (hi as u16) << 7, v);
        }
    }

    /// What the built bundle does in a host, which no unit test of the
    /// engine can answer: the preset parameter reaches the engine, the
    /// patch survives a state round trip, and the sustain pedal holds.
    #[test]
    #[ignore = "needs Hymnos.clap installed"]
    fn hymnos_answers_its_host() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let path = PathBuf::from(home).join(".clap").join("Hymnos.clap");
        if !path.exists() {
            eprintln!("Hymnos.clap is not installed; skipping");
            return;
        }
        let load = || ClapPlugin::load(&path, None, 48_000.0, 512).expect("load");
        let preset_id = load().params().iter().find(|p| p.name == "Preset").expect("a Preset parameter").id;
        let preset = load().params().iter().find(|p| p.name == "Preset").cloned().expect("a Preset parameter");
        eprintln!("preset parameter: {} .. {} (default {})", preset.min, preset.max, preset.default);
        let (lo, hi) = (preset.min as f32, preset.max as f32);
        assert!(hi - lo >= 20.0, "the bank shrank: {lo}..{hi}");

        let peak_of = |p: &mut ClapPlugin, hold: usize| -> f32 {
            let mut out = vec![0.0f32; 512 * 2];
            for _ in 0..4 { p.process(None, &mut out, 512); }
            p.note_on(60, 110);
            let mut peak = 0.0f32;
            for _ in 0..hold {
                p.process(None, &mut out, 512);
                for s in &out { peak = peak.max(s.abs()); }
            }
            p.note_off(60);
            for _ in 0..40 { p.process(None, &mut out, 512); }
            peak
        };

        // Every preset the parameter names is audible through the wrapper.
        for i in 1..=3i32 {
            let mut p = load();
            p.set_param_normalized(preset_id, (i as f32 - lo) / (hi - lo));
            let peak = peak_of(&mut p, 40);
            assert!(peak > 0.01, "preset {i} is silent in a host: {peak}");
            assert!(peak <= 1.0, "preset {i} is not clamped: {peak}");
        }

        // A state round trip renders the same thing.
        let mut p = load();
        p.set_param_normalized(preset_id, (5.0 - lo) / (hi - lo));
        let mut out = vec![0.0f32; 512 * 2];
        for _ in 0..4 { p.process(None, &mut out, 512); }
        let saved = p.save_state();
        let before = peak_of(&mut p, 40);
        let mut q = load();
        assert!(q.load_state(&saved), "the state was refused");
        let after = peak_of(&mut q, 40);
        assert!((after - before).abs() < before * 0.05,
            "the restored state renders differently: {after:.4} vs {before:.4}");

        // The pedal holds what the keys let go.
        let mut p = load();
        p.set_param_normalized(preset_id, (13.0 - lo) / (hi - lo)); // a stab, short release
        let mut out = vec![0.0f32; 512 * 2];
        for _ in 0..4 { p.process(None, &mut out, 512); }
        p.set_param(64, 0.0);
        let dry = {
            p.note_on(60, 110);
            for _ in 0..8 { p.process(None, &mut out, 512); }
            p.note_off(60);
            let mut peak = 0.0f32;
            for _ in 0..30 {
                p.process(None, &mut out, 512);
                for s in &out { peak = peak.max(s.abs()); }
            }
            peak
        };
        eprintln!("hymnos: preset round trip {before:.4} -> {after:.4}, tail after note off {dry:.4}");
    }

    /// A plugin of this repository built as a shared object: the FX rack,
    /// an effect, or Pulsar, an instrument. Built on demand.
    /// The CLAP twin of the VST3 host's test: a saved state whose compressor
    /// was pushed into saturation, then a preset change from the host, must
    /// render the preset's level and not the edit's.
    #[test]
    #[ignore]
    fn a_preset_change_puts_the_edited_chain_back() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let path = PathBuf::from(home).join(".clap").join("Cordis.clap");
        if !path.exists() {
            eprintln!("Cordis.clap is not installed; skipping");
            return;
        }
        let load = || ClapPlugin::load(&path, None, 48_000.0, 512).expect("load");
        let preset_id = load().params().iter().find(|p| p.name == "Preset").expect("a Preset parameter").id;
        let second = 2.0 / 10.0;
        let peak_of = |p: &mut ClapPlugin| -> f32 {
            let mut out = vec![0.0f32; 512 * 2];
            for _ in 0..4 { p.process(None, &mut out, 512); }
            p.note_on(48, 120);
            let mut peak = 0.0f32;
            for _ in 0..60 {
                p.process(None, &mut out, 512);
                for s in &out { peak = peak.max(s.abs()); }
            }
            p.note_off(48);
            for _ in 0..80 { p.process(None, &mut out, 512); }
            peak
        };

        let reference = {
            let mut p = load();
            p.set_param_normalized(preset_id, second);
            peak_of(&mut p)
        };

        let mut p = load();
        p.set_param_normalized(preset_id, 1.0 / 10.0);
        let mut out = vec![0.0f32; 512 * 2];
        for _ in 0..4 { p.process(None, &mut out, 512); }
        // The wrapper writes a u64 length, then JSON, zstd-compressed when
        // it was built so.
        let raw = p.save_state();
        let declared = u64::from_le_bytes(raw[..8].try_into().unwrap()) as usize;
        let body = &raw[8..];
        let compressed = body.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]);
        let json = if compressed { zstd::decode_all(body).expect("zstd state") } else { body.to_vec() };
        eprintln!("state: {} bytes, {declared} declared, compressed {compressed}", raw.len());
        let mut state: serde_json::Value = serde_json::from_slice(&json).expect("state is JSON");
        let mut patch: serde_json::Value = serde_json::from_str(state["fields"]["patch"].as_str().expect("the patch field")).expect("the patch is JSON");
        patch["fx"]["slots"][1]["params"]["threshold"] = serde_json::json!(-60.0);
        patch["fx"]["slots"][1]["params"]["makeup"] = serde_json::json!(24.0);
        state["fields"]["patch"] = serde_json::Value::String(patch.to_string());
        let body = state.to_string().into_bytes();
        let body = if compressed { zstd::encode_all(body.as_slice(), 0).expect("zstd") } else { body };
        let mut bytes = (body.len() as u64).to_le_bytes().to_vec();
        bytes.extend_from_slice(&body);
        assert!(p.load_state(&bytes), "the state was refused");
        let pushed = peak_of(&mut p);

        p.set_param_normalized(preset_id, second);
        let after = peak_of(&mut p);

        eprintln!("reference {reference:.4}, pushed {pushed:.4}, after the preset change {after:.4}");
        assert!(pushed > reference * 2.0, "the pushed compressor did not saturate: {pushed:.4} vs {reference:.4}");
        assert!((after - reference).abs() < reference * 0.05, "the chain kept the edit: {after:.4} vs {reference:.4}");
    }

    /// An installed CLAP that takes audio in, or nothing.
    ///
    /// This crate ships no plugin of its own, and building one from a sibling
    /// repository would make its tests depend on a tree that may not be
    /// there. So the tests below run against what is installed and say so
    /// when there is nothing to run against.
    fn an_installed_effect() -> Option<ClapPluginInfo> {
        scan_clap().into_iter().find(|p| p.is_effect)
    }

    /// An installed CLAP instrument, or nothing.
    fn an_installed_instrument() -> Option<ClapPluginInfo> {
        scan_clap().into_iter().find(|p| p.is_instrument)
    }

    #[test]
    fn an_effect_scans_loads_and_passes_audio() {
        let Some(info) = an_installed_effect() else {
            eprintln!("no CLAP effect installed; skipping");
            return;
        };
        let so = PathBuf::from(&info.path);
        let infos = scan_clap_file(&so).unwrap();
        assert!(!infos.is_empty());
        let mut p = ClapPlugin::load(&so, Some(&info.id), 48_000.0, 256).unwrap();
        assert!(p.has_audio_input());
        assert!(!p.params().is_empty());
        let input: Vec<f32> = (0..512).map(|i| ((i / 2) as f32 * 0.01).sin() * 0.5).collect();
        let mut out = vec![0.0f32; 512];
        for _ in 0..4 { p.process(Some(&input), &mut out, 256); }
        let energy: f32 = out.iter().map(|x| x * x).sum();
        assert!(energy > 1.0, "an empty rack passes the signal: {energy}");
        // The host's half of the state round trip: the stream it writes is
        // the stream it reads back. Whether a given plugin accepts its own
        // state is that plugin's business, and at least one installed here
        // must, or the reader is what is broken.
        let state = p.save_state();
        assert!(!state.is_empty(), "{} saved nothing", info.name);
        let round_trips: Vec<(String, bool)> = scan_clap().into_iter()
            .filter(|i| i.is_effect)
            .filter_map(|i| ClapPlugin::load(
                &PathBuf::from(&i.path), Some(&i.id), 48_000.0, 256).ok().map(|p| (i.name, p)))
            .map(|(name, mut p)| { let st = p.save_state(); let ok = p.load_state(&st); (name, ok) })
            .collect();
        assert!(round_trips.iter().any(|(_, ok)| *ok),
                "no installed effect reloaded its own state, so the reader is: {round_trips:?}");
        let first = p.params()[0].clone();
        p.set_param_normalized(first.id, 1.0);
        p.process(Some(&input), &mut out, 256);
        let v = p.get_param(first.id).unwrap();
        assert!((v - first.max).abs() < 1e-6 || first.stepped, "{v} vs {}", first.max);
    }

    #[test]
    fn an_instrument_plays_a_note() {
        // Across every instrument installed, not the first one: what is
        // asserted is that the HOST can make one sound -- open it, send a
        // note, get audio back. Whether a particular instrument answers a
        // bare middle C in ten blocks is that instrument's business, and
        // some (a bowed string, a talkbox with no line to sing) legitimately
        // do not.
        let instruments: Vec<ClapPluginInfo> =
            scan_clap().into_iter().filter(|p| p.is_instrument).collect();
        if instruments.is_empty() {
            eprintln!("no CLAP instrument installed; skipping");
            return;
        }
        let mut sounded: Vec<&str> = Vec::new();
        for info in &instruments {
            let Ok(mut p) = ClapPlugin::load(
                &PathBuf::from(&info.path), Some(&info.id), 48_000.0, 256) else { continue };
            let mut out = vec![0.0f32; 512];
            p.process(None, &mut out, 256);
            let quiet: f32 = out.iter().map(|x| x.abs()).sum();
            p.note_on(60, 100);
            let mut loud = 0.0f32;
            for _ in 0..40 {
                p.process(None, &mut out, 256);
                loud += out.iter().map(|x| x.abs()).sum::<f32>();
            }
            p.all_notes_off();
            if loud > quiet + 1.0 { sounded.push(&info.name); }
        }
        assert!(!sounded.is_empty(),
                "not one of {} installed instruments made a sound", instruments.len());
        eprintln!("{} of {} instruments sounded: {sounded:?}", sounded.len(), instruments.len());
    }
}
