pub mod code_action;
pub mod coordinates;
pub mod db;
mod declarations;
pub mod diagnostics;
pub mod diagnostics_syntax;
#[cfg(any(test, feature = "testing"))]
pub mod fixtures;
pub mod folding_range;
pub mod indent;
pub mod indexer;
pub mod selection_range;
pub mod symbols;
pub mod traits;
pub mod treesitter;
