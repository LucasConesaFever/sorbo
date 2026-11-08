//! Running a callback's work on the shared state without ever aborting sorbo (HARD-6).
//!
//! macOS calls sorbo back from C (timers, Accessibility notices, hotkeys, clicks). Two things
//! there would end the process: borrowing the state while it's already borrowed (a callback
//! fired from inside a nested run loop, such as a menu being tracked), and a panic unwinding
//! into C. `run_guarded` turns both into values the caller can act on.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

/// What happened to a callback's work.
#[derive(Debug, PartialEq, Eq)]
pub enum Ran<R> {
    /// It ran and returned this.
    Done(R),
    /// The state was already borrowed: nothing ran. The caller retries or queues the work.
    Busy,
    /// It panicked. The panic message went to the log (the default panic hook writes it to
    /// stderr, which is the log); the borrow was released, so sorbo carries on.
    Panicked,
}

/// When a timer callback should fire again after `ran` (HARD-6). Busy: shortly, the borrow
/// will be gone. Panicked: in a second, because many timers only re-arm themselves at the end
/// of their work, and one panic must not stop them for good (a stopped lost-window timer
/// would never report those windows gone). Done: the work re-armed it if it needed to.
pub fn retry_after<R>(ran: &Ran<R>) -> Option<Duration> {
    match ran {
        Ran::Busy => Some(Duration::from_millis(10)),
        Ran::Panicked => Some(Duration::from_secs(1)),
        Ran::Done(_) => None,
    }
}

/// Runs `f` on the value in `cell`, unless it's already borrowed, catching any panic.
///
/// `AssertUnwindSafe`: after a panic the state may be half-updated. That is accepted: a
/// window manager that keeps running with one stale entry is better than one that quits and
/// leaves windows parked off screen.
pub fn run_guarded<T, R>(cell: &RefCell<T>, f: impl FnOnce(&mut T) -> R) -> Ran<R> {
    let Ok(mut value) = cell.try_borrow_mut() else {
        return Ran::Busy;
    };
    match catch_unwind(AssertUnwindSafe(|| f(&mut value))) {
        Ok(r) => Ran::Done(r),
        Err(_) => Ran::Panicked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_6_timer_retries_when_busy_or_after_a_panic() {
        assert_eq!(
            retry_after(&Ran::<()>::Busy),
            Some(Duration::from_millis(10))
        );
        assert_eq!(
            retry_after(&Ran::<()>::Panicked),
            Some(Duration::from_secs(1))
        );
        assert_eq!(retry_after(&Ran::Done(())), None);
    }

    #[test]
    fn hard_6_runs_when_free() {
        let cell = RefCell::new(1);
        assert_eq!(
            run_guarded(&cell, |n| {
                *n += 1;
                *n
            }),
            Ran::Done(2)
        );
    }

    #[test]
    fn hard_6_busy_state_is_reported_not_a_panic() {
        let cell = RefCell::new(1);
        let _held = cell.borrow_mut();
        assert_eq!(run_guarded(&cell, |n| *n), Ran::Busy);
    }

    #[test]
    fn hard_6_panic_is_caught_and_the_state_stays_usable() {
        let cell = RefCell::new(1);
        let ran: Ran<()> = run_guarded(&cell, |_| panic!("boom"));
        assert_eq!(ran, Ran::Panicked);
        assert_eq!(run_guarded(&cell, |n| *n), Ran::Done(1));
    }
}
