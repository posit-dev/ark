//
// thread.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::thread::ThreadId;

static R_MAIN_THREAD_ID: OnceLock<ThreadId> = OnceLock::new();
static R_INITIALIZED: AtomicBool = AtomicBool::new(false);
static TEST_INIT_HOOK: OnceLock<fn()> = OnceLock::new();

/// Called once by whoever owns the R thread (Console in Ark, the headless R
/// thread in Oak, or `test_init()` in unit tests).
pub fn set_r_main_thread() {
    if R_MAIN_THREAD_ID.set(std::thread::current().id()).is_err() {
        panic!("`set_r_main_thread()` can only be called once");
    }
}

/// Returns `true` if the calling thread is the R main thread.
///
/// Returns `false` if `set_r_main_thread()` has not been called yet.
pub fn on_r_main_thread() -> bool {
    R_MAIN_THREAD_ID
        .get()
        .is_some_and(|id| *id == std::thread::current().id())
}

/// Returns `true` once the R session is fully initialized, i.e. the event
/// loop consumer (Console or Oak) is ready to process tasks.
///
/// This is NOT set during test init — unit tests always use the test escape
/// path in `r_task()`.
pub fn is_r_initialized() -> bool {
    R_INITIALIZED.load(Ordering::Acquire)
}

/// Mark R as fully initialized. Called by `Console::complete_initialization()`
/// in Ark and by Oak's headless R thread after setup.
pub fn set_r_initialized() {
    R_INITIALIZED.store(true, Ordering::Release);
}

/// Register an additional init function to run during test init.
///
/// Ark uses this to register `modules::initialize()` so that its R modules
/// are loaded before unit tests run.
pub fn set_test_init_hook(hook: fn()) {
    TEST_INIT_HOOK.set(hook).ok();
}

/// Perform test-time R initialization.
///
/// Calls `harp::fixtures::r_test_init()` for base R setup, then
/// `set_r_main_thread()`, then the downstream hook (if registered).
///
/// Guarded by `Once` so it is safe to call repeatedly.
#[cfg(feature = "testing")]
pub(crate) fn test_init() {
    use std::sync::Once;
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        harp::fixtures::r_test_init();
        set_r_main_thread();
        if let Some(hook) = TEST_INIT_HOOK.get() {
            hook();
        }
    });
}
