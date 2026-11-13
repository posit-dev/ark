//! Tree-sitter-based IDE features and helpers for R.
//!
//! This crate is a stopgap. Oak's engine, `oak_ide`, answers from Air's syntax
//! tree and the semantic index, and already provides navigation and rename.
//! Porting a feature means switching its handler in `oak_lsp` to `oak_ide` and
//! deleting its code here; the crate goes away once nothing is left. The
//! features still answered here:
//!
//! - Document and workspace symbols ([`symbols`]).
//! - Folding ranges ([`folding_range`]).
//! - Selection ranges ([`selection_range`]).
//! - On-type indentation ([`indent`]).
//! - Code actions ([`code_action`]).
//! - Diagnostics ([`diagnostics`], [`diagnostics_syntax`], `declarations`).
//! - The workspace symbol index ([`indexer`]), also used by Ark's workspace
//!   completions.
//!
//! Ark's completions, hover and signature help also use the tree-sitter
//! helpers in [`treesitter`], [`traits`] and [`coordinates`], and the parse
//! query exposed through [`db::FileLegacyExt::tree_sitter()`].
//!
//! This crate does not depend on `oak_lsp`. Its dependency graph must also
//! exclude `ark`, `harp` and `libr`, which provide the embedded R session.
//! `scripts/check-crate-graph` checks this exclusion, including build and
//! dev-dependencies, with all features and for all target platforms.

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
