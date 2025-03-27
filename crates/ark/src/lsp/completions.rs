//
// completions.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

mod completion_context;
mod completion_item;
mod parameter_hints;
mod provide;
mod resolve;
mod sources;
mod types;

pub(crate) use provide::provide_completions;
pub(crate) use resolve::resolve_completion;
