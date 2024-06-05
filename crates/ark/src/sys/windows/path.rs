/*
 * path.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use std::path::PathBuf;

pub fn r_user_home() -> Option<PathBuf> {
    std::env::var("R_USER").ok().map(PathBuf::from)
}
