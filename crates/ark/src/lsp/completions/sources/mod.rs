//
// mod.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

mod call;
mod comment;
mod composite;
mod custom;
mod document;
mod extractor;
mod file_path;
mod keyword;
mod names;
mod namespace;
mod pipe;
mod search_path;
mod subset;
mod utils;
mod workspace;

pub use comment::completions_from_comment;
pub use composite::completions_from_composite_sources;
pub use custom::completions_from_custom_source;
pub use extractor::completions_from_at;
pub use extractor::completions_from_dollar;
pub use file_path::completions_from_file_path;
pub use namespace::completions_from_namespace;
