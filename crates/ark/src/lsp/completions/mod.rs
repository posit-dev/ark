//
// mod.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

mod completion_item;
mod provide;
mod resolve;
mod sources;
mod types;

pub use provide::provide_completions;
pub use resolve::resolve_completion;
