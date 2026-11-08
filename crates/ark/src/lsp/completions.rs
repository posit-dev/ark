//
// completions.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

// `static_completions()` recognizes the context and builds an owned
// `RCompletionRequest` for session-dependent sources without calling R.
// `r_completions()` answers that request on the R thread, and `finish()` merges
// the replies with the static items. Of the modules that produce completions,
// only `r_completions` and its submodules call R. `resolve` also calls R, to
// render help for the selected item.

mod completion_context;
mod completion_item;
mod function_context;
mod provide;
mod r_completions;
mod request;
mod resolve;
mod sources;
mod types;

#[cfg(test)]
mod tests;

pub(crate) use provide::finish;
pub(crate) use provide::static_completions;
pub(crate) use r_completions::r_completions;
pub(crate) use request::RCompletionReplies;
pub(crate) use resolve::resolve_completion;
pub(crate) use resolve::resolve_target;
