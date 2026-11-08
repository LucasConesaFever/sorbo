//! Applying the config file at startup and again on every save (CFG-1, CFG-4, spec 5).
//!
//! FSEvents watches the file's directory, not the file: editors save by writing a new file
//! and renaming it over the old one, and a watch on the old file would never see that.
//! FSEvents' file-level events also catch editors that write in place. Every event re-arms
//! a 200 ms timer, so the several writes of one save become one reload; the reload itself
//! does nothing when the bytes are the same as last time.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::Duration;

use objc2_core_foundation::{CFArray, CFRetained, CFRunLoopTimer, CFString};
use wm_core::Event;

use super::{Shared, arm, hotkeys, make_timer, run_or_retry};
use crate::config_load::{Loaded, load, log_lines, unchanged, watch_dirs};
use crate::wm_log;

/// Quiet time after the last file event before reloading.
const DEBOUNCE: Duration = Duration::from_millis(200);
/// How long FSEvents may hold events back to batch them (seconds).
const FSEVENTS_LATENCY: f64 = 0.05;

type FSEventStreamRef = *mut c_void;
type FSEventStreamCallback = unsafe extern "C" fn(
    stream: FSEventStreamRef,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
);

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

const K_FS_EVENT_STREAM_EVENT_ID_SINCE_NOW: u64 = u64::MAX;
/// Report changes to files inside the directory, not only "something changed here".
const K_FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS: u32 = 0x10;

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: FSEventStreamCallback,
        context: *const FSEventStreamContext,
        paths: *const c_void,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> FSEventStreamRef;
    fn FSEventStreamSetDispatchQueue(stream: FSEventStreamRef, queue: *const c_void);
    fn FSEventStreamStart(stream: FSEventStreamRef) -> u8;
}

// libdispatch is part of libSystem, which every program links.
unsafe extern "C" {
    /// The main queue (`dispatch_get_main_queue()` is a macro for its address). The main
    /// run loop drains it, so the callback runs on the main thread while `pump` runs.
    static _dispatch_main_q: u8;
}

/// Loads the file, registers its bindings, queues its gaps (before any monitor event, so
/// the first layout already uses them) and starts watching for saves.
pub(crate) fn install(shared: &'static RefCell<Shared>) -> Result<(), String> {
    let timer = make_timer(shared, on_reload_timer)?;
    // The FSEvents callback arms the timer directly, without borrowing the shared state, so
    // a save is never lost because the state was busy. `Shared` keeps the timer alive for
    // the rest of the process, so this pointer stays valid.
    let timer_ptr = CFRetained::as_ptr(&timer).as_ptr().cast::<c_void>();
    let path = {
        let mut s = shared.borrow_mut();
        s.config_timer = Some(timer);
        let loaded = load(&s.config_path, None);
        apply(&mut s, loaded, true);
        s.config_path.clone()
    };
    let dir = path.parent().ok_or("the config path has no directory")?;
    // Watching needs the directory to exist; creating it lets a file made later be seen.
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("can't create {}: {e}; saves won't reload", dir.display()))?;
    // A symlinked file (dotfiles repo) is edited where it really lives: watch there too.
    let dirs = watch_dirs(&path);
    for dir in &dirs {
        wm_log!("config: watching {}", dir.display());
    }
    let names: Vec<_> = dirs
        .iter()
        .map(|d| CFString::from_str(&d.to_string_lossy()))
        .collect();
    let name_refs: Vec<&CFString> = names.iter().map(|n| &**n).collect();
    let paths = CFArray::<CFString>::from_objects(&name_refs);
    let context = FSEventStreamContext {
        version: 0,
        info: timer_ptr,
        retain: std::ptr::null(),
        release: std::ptr::null(),
        copy_description: std::ptr::null(),
    };
    // SAFETY: `context` and `paths` are valid for the call (FSEvents copies both); `info`
    // is the reload timer, which `Shared` keeps for the life of the process.
    let stream = unsafe {
        FSEventStreamCreate(
            std::ptr::null(),
            on_fs_event,
            &context,
            (&*paths as *const CFArray<CFString>).cast(),
            K_FS_EVENT_STREAM_EVENT_ID_SINCE_NOW,
            FSEVENTS_LATENCY,
            K_FS_EVENT_STREAM_CREATE_FLAG_FILE_EVENTS,
        )
    };
    if stream.is_null() {
        return Err("FSEventStreamCreate failed; saves won't reload".into());
    }
    // SAFETY: a valid stream and the main queue; the stream is never stopped or released,
    // so it watches for the rest of the process.
    let started = unsafe {
        FSEventStreamSetDispatchQueue(stream, (&raw const _dispatch_main_q).cast());
        FSEventStreamStart(stream)
    };
    if started == 0 {
        return Err("FSEventStreamStart failed; saves won't reload".into());
    }
    Ok(())
}

/// Something changed in the config directory: reload once things are quiet.
unsafe extern "C" fn on_fs_event(
    _stream: FSEventStreamRef,
    info: *mut c_void,
    _count: usize,
    _paths: *mut c_void,
    _flags: *const u32,
    _ids: *const u64,
) {
    // SAFETY: `info` is the reload timer given in `install`, kept alive by `Shared`.
    if let Some(timer) = unsafe { info.cast::<CFRunLoopTimer>().as_ref() } {
        arm(timer, DEBOUNCE);
    }
}

unsafe extern "C-unwind" fn on_reload_timer(timer: *mut CFRunLoopTimer, info: *mut c_void) {
    // SAFETY: `info` is the shared-state pointer given to `make_timer`; a busy WM retries
    // shortly, so a save is never lost.
    unsafe { run_or_retry(timer, info, "config reload", reload) };
}

fn reload(s: &mut Shared) {
    if unchanged(&s.config_text, &s.config_path) {
        return;
    }
    let loaded = load(&s.config_path, Some(&s.config));
    apply(s, loaded, false);
}

/// Puts `loaded` into effect: logs it, re-registers the hotkeys if the bindings changed,
/// sends new gaps to core, and updates the error set.
fn apply(s: &mut Shared, loaded: Loaded, first: bool) {
    for line in log_lines(&s.config_path, &loaded) {
        wm_log!("{line}");
    }
    if first || loaded.config.bindings != s.config.bindings {
        s.refused = hotkeys::apply_bindings(s, &loaded.config.bindings);
    }
    if first || loaded.config.gaps != s.config.gaps {
        s.push(Event::GapsChanged(loaded.config.gaps));
    }
    let mut errors = loaded.errors;
    errors.extend(s.refused.iter().cloned());
    s.config = loaded.config;
    s.config_text = loaded.text;
    s.config_errors = errors;
    config_errors_changed(s);
}

/// The config's error set may have changed: the banner and the menu bar dot follow it
/// (CFG-6). The errors themselves are already in the log.
fn config_errors_changed(s: &mut Shared) {
    s.banner_state.set_config_errors(s.config_errors.clone());
    super::banner::refresh(s);
}
