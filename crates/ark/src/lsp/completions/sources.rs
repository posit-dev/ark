//
// sources.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

mod common;
mod composite;
mod unique;
mod utils;

pub use composite::completions_from_composite_sources;
pub use unique::completions_from_unique_sources;
