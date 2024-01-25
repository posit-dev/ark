//
// sys.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

cfg_if::cfg_if! {
    if #[cfg(unix)] {
        mod unix;
        pub use self::unix::*;
    } else if #[cfg(windows)] {
        mod windows;
        pub use self::windows::*;
    }
}
