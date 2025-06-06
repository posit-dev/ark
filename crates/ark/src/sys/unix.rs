/*
 * unix.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

pub mod console;
pub mod control;
pub mod interface;
pub mod path;
pub mod signals;
pub mod traps;

cfg_if::cfg_if! {
    if #[cfg(target_os = "linux")] {
        mod linux;
        pub use self::linux::*;
    } else if #[cfg(target_os = "macos")] {
        mod macos;
        pub use self::macos::*;
    }
}
