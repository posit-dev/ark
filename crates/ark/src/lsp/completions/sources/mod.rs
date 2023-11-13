//
// mod.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

mod call;
mod comment;
mod custom;
mod document;
mod extractor;
mod file_path;
mod general;
mod keyword;
mod names;
mod namespace;
mod search_path;
mod utils;
mod workspace;

pub use call::completions_from_call;
pub use comment::completions_from_comment;
pub use custom::completions_from_custom_source;
pub use extractor::completions_from_at;
pub use extractor::completions_from_dollar;
pub use file_path::completions_from_file_path;
pub use general::completions_from_general_sources;
pub use namespace::completions_from_namespace;
