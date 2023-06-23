//
// lock.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::sync::atomic::AtomicI32;

use libR_sys::R_interrupts_suspended;
use log::info;
use parking_lot::ReentrantMutex;

// The R lock is to be used by threads which wish to access the R runtime. The
// main thread owns access to the R runtime by default, but it can yield access
// to other threads through the use of the facilities in this module.
//
// Threads will then be given an opportunity to execute code in the next call
// made by R to the R_PolledEvents event handler, which happens quite frequently
// (usually via R_ProcessEvents).

extern "C" {
    pub static mut R_PolledEvents: Option<unsafe extern "C" fn()>;
}

#[no_mangle]
pub extern "C" fn r_polled_events_disabled() {}

// The R runtime lock, used to synchronize access to R. A ReentrantMutex
// is used to ensure we can call `r_lock!` from within an `r_lock!` without
// causing a deadlock.
pub static mut R_RUNTIME_LOCK: ReentrantMutex<()> = ReentrantMutex::new(());
pub static R_RUNTIME_LOCK_COUNT: AtomicI32 = AtomicI32::new(0);
pub static R_RUNTIME_NEST_LEVEL: AtomicI32 = AtomicI32::new(0);

pub fn initialize() {}

pub fn with_r_lock<T, F: FnMut() -> T>(callback: F) -> T {
    unsafe { with_r_lock_impl(callback) }
}

// Because we use a ReentrantMutex to take the lock, it is important to ensure
// that everything inside `with_r_lock_impl()` can be called in a recursive
// manner. Resetting `R_PolledEvents` and `R_interrupts_suspended` to their "old"
// values (rather than to a default value) on the way out is particularly important.
pub unsafe fn with_r_lock_impl<T, F: FnMut() -> T>(mut callback: F) -> T {
    let level = R_RUNTIME_NEST_LEVEL.load(std::sync::atomic::Ordering::Acquire);

    // Let the logger know we're taking the lock.
    let id = std::thread::current().id();
    info!(
        "{:?} (nest level {}) is requesting R runtime lock.",
        id, level
    );

    // Record how long it takes the acquire the lock.
    let now = std::time::SystemTime::now();

    // Let the main thread know we're waiting for the lock.
    // Do so by ncrementing the count for number of waiting threads.
    R_RUNTIME_LOCK_COUNT.fetch_add(1, std::sync::atomic::Ordering::AcqRel);

    // Start waiting for the lock.
    let guard = R_RUNTIME_LOCK.lock();

    // If we get here, we now have the lock, so decrement the count.
    R_RUNTIME_LOCK_COUNT.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);

    // Log how long we were stuck waiting.
    let elapsed = now.elapsed().unwrap().as_millis();
    info!(
        "{:?} (nest level {}) obtained lock after waiting for {} milliseconds.",
        id, level, elapsed
    );

    // Increment the nest level to track when we call `r_lock!` from within `r_lock!`.
    R_RUNTIME_NEST_LEVEL.fetch_add(1, std::sync::atomic::Ordering::AcqRel);

    // Disable polled events in this scope.
    let polled_events = unsafe { R_PolledEvents };
    R_PolledEvents = Some(r_polled_events_disabled);

    // Disable interrupts in this scope.
    let interrupts_suspended = R_interrupts_suspended;
    R_interrupts_suspended = 1;

    // Execute the callback.
    let now = std::time::SystemTime::now();
    let result = callback();

    // Restore interrupts.
    R_interrupts_suspended = interrupts_suspended;

    // Restore the polled events handler.
    R_PolledEvents = polled_events;

    // Decrement the nest level.
    R_RUNTIME_NEST_LEVEL.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);

    // Release the runtime lock.
    drop(guard);

    // Let the logger know we've released the lock..
    let elapsed = now.elapsed().unwrap().as_millis();
    info!(
        "{:?} (nest level {}) has released the R runtime lock after {} milliseconds.",
        id, level, elapsed
    );

    // Return the resulting expression.
    result
}
