//! Suppresses property panic output while preserving its message and location.
//!
//! Mutator panics occur outside [`catch_quietly()`] and still reach the
//! previously installed hook.

use std::cell::Cell;
use std::cell::RefCell;
use std::panic::AssertUnwindSafe;
use std::panic::PanicHookInfo;
use std::sync::Arc;

thread_local! {
    static QUIET: Cell<bool> = const { Cell::new(false) };
    static RECORDED: RefCell<Option<String>> = const { RefCell::new(None) };
}

type Hook = dyn for<'info> Fn(&PanicHookInfo<'info>) + Send + Sync + 'static;

pub(crate) struct Guard {
    previous: Option<Arc<Hook>>,
}

/// Keep the returned [`Guard`] alive while running scenarios to capture panic
/// messages and locations. The hook records panics inside `catch_quietly()` on
/// the calling thread and delegates all others to the previous hook.
pub(crate) fn install() -> Guard {
    let previous: Arc<Hook> = Arc::from(std::panic::take_hook());
    let delegate = Arc::clone(&previous);
    std::panic::set_hook(Box::new(move |info| {
        if !QUIET.with(Cell::get) {
            delegate(info);
            return;
        }
        RECORDED.with(|recorded| *recorded.borrow_mut() = Some(describe(info)));
    }));
    Guard {
        previous: Some(previous),
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // `take_hook()` panics during unwinding. Keep the wrapper installed,
        // where it delegates whenever `catch_quietly()` is inactive.
        if std::thread::panicking() {
            return;
        }
        let _ = std::panic::take_hook();
        let Some(previous) = self.previous.take() else {
            return;
        };
        std::panic::set_hook(Box::new(move |info| previous(info)));
    }
}

/// Returns the panic's message and location, which [`std::panic::catch_unwind`]
/// alone cannot recover.
pub(super) fn catch_quietly(body: impl FnOnce()) -> std::result::Result<(), String> {
    RECORDED.with(|recorded| *recorded.borrow_mut() = None);
    QUIET.with(|quiet| quiet.set(true));
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(body));
    QUIET.with(|quiet| quiet.set(false));

    if outcome.is_ok() {
        return Ok(());
    }
    match RECORDED.with(|recorded| recorded.borrow_mut().take()) {
        Some(recorded) => Err(recorded),
        None => Err("panicked without reaching the panic hook".to_string()),
    }
}

fn describe(info: &PanicHookInfo<'_>) -> String {
    let location = match info.location() {
        Some(location) => location.to_string(),
        None => "unknown location".to_string(),
    };
    let payload = info.payload_as_str().unwrap_or("Box<dyn Any>");
    format!("panicked at {location}: {payload}")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;

    use super::*;

    #[test]
    fn test_guard_restores_previous_hook() {
        let original = std::panic::take_hook();
        let called = Arc::new(AtomicBool::new(false));
        let hook_called = Arc::clone(&called);
        std::panic::set_hook(Box::new(move |_| {
            hook_called.store(true, Ordering::SeqCst);
        }));

        {
            let _guard = install();
        }
        let outcome = std::panic::catch_unwind(|| panic!("test panic"));

        let _ = std::panic::take_hook();
        std::panic::set_hook(original);

        assert!(outcome.is_err());
        assert!(called.load(Ordering::SeqCst));
    }
}
