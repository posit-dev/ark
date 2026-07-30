//
// panic.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::any::Any;

/// The message out of a panic payload, for logging. Takes `&dyn Any` so it
/// serves both a `catch_unwind()` payload and `PanicHookInfo::payload()`.
///
/// `panic!()` boxes its message as `&str` when it has no arguments to format
/// and as `String` when it does, so both need handling. Anything else comes
/// from `panic_any()`.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        msg.to_string()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        String::from("(unknown panic payload)")
    }
}

#[cfg(test)]
mod tests {
    use super::panic_message;

    #[test]
    fn test_panic_message_handles_both_payload_types() {
        let unformatted = std::panic::catch_unwind(|| panic!("plain")).unwrap_err();
        assert_eq!(panic_message(unformatted.as_ref()), "plain");

        let formatted = std::panic::catch_unwind(|| panic!("formatted {}", 1)).unwrap_err();
        assert_eq!(panic_message(formatted.as_ref()), "formatted 1");

        let other = std::panic::catch_unwind(|| std::panic::panic_any(42u8)).unwrap_err();
        assert_eq!(panic_message(other.as_ref()), "(unknown panic payload)");
    }
}
