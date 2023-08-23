//
// result.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

pub trait ResultExt<T, E> {
    /// Calls the provided closure with the contained error (if [`Err`]).
    ///
    /// Consumes the Result, unlike `inspect_err()` which propagates it and
    /// still requires you to handle the Result in some way.
    fn on_err<F: FnOnce(E)>(self, f: F);
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn on_err<F: FnOnce(E)>(self, f: F) {
        if let Err(e) = self {
            f(e);
        }
    }
}
