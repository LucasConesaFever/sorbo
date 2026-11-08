//! Quitting cleanly on Ctrl+C / SIGTERM (spec 7.4).
//!
//! A plain `signal()` handler may only call a tiny set of functions, and stopping the run
//! loop isn't one of them. A GCD signal source instead runs our handler as an ordinary
//! callback on the main queue, which the run loop drains. GCD lives in libSystem (always
//! linked), so a few hand-written declarations are enough: no new dependency.

use std::cell::RefCell;
use std::ffi::c_void;

use super::{Shared, info_ptr, run_or_defer};
use crate::wm_log;

const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;
/// `SIG_IGN`: ignore the signal's default action (GCD still sees it).
const SIG_IGN: usize = 1;

unsafe extern "C" {
    /// What the C macro `dispatch_get_main_queue()` points at.
    static _dispatch_main_q: c_void;
    /// What the C macro `DISPATCH_SOURCE_TYPE_SIGNAL` points at.
    static _dispatch_source_type_signal: c_void;
    fn dispatch_source_create(
        kind: *const c_void,
        handle: usize,
        mask: usize,
        queue: *const c_void,
    ) -> *mut c_void;
    fn dispatch_set_context(object: *mut c_void, context: *mut c_void);
    fn dispatch_source_set_event_handler_f(
        source: *mut c_void,
        handler: extern "C" fn(*mut c_void),
    );
    fn dispatch_resume(object: *mut c_void);
    fn signal(signum: i32, handler: usize) -> usize;
}

/// Makes SIGINT and SIGTERM stop the WM loop instead of killing the process, so `main` can
/// restore hidden windows.
pub(crate) fn install(shared: &'static RefCell<Shared>) {
    let context = info_ptr(shared);
    for signum in [SIGINT, SIGTERM] {
        // SAFETY: ignoring a signal's default action has no preconditions.
        unsafe { signal(signum, SIG_IGN) };
        // SAFETY: both statics are provided by libSystem; the queue is the main queue.
        let source = unsafe {
            dispatch_source_create(
                &raw const _dispatch_source_type_signal,
                signum as usize,
                0,
                &raw const _dispatch_main_q,
            )
        };
        if source.is_null() {
            wm_log!("could not watch signal {signum}; Ctrl+C will not restore windows");
            continue;
        }
        // SAFETY: `source` is a fresh dispatch source; `context` is the shared-state pointer,
        // valid as long as `MacPlatform` lives. The source is never released: it lives as
        // long as the process.
        unsafe {
            dispatch_set_context(source, context);
            dispatch_source_set_event_handler_f(source, on_signal);
            dispatch_resume(source);
        }
    }
}

/// Runs on the main queue when SIGINT or SIGTERM arrives.
extern "C" fn on_signal(context: *mut c_void) {
    wm_log!("quit signal received");
    // SAFETY: `context` is the shared-state pointer set in `install`.
    unsafe {
        run_or_defer(context, "quit signal", |s| {
            s.quit = true;
            s.wake();
        })
    };
}
