/*
 * platform.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(target_os = "windows"))]
mod unix;
#[cfg(not(target_os = "windows"))]
pub use unix::*;
