//! Exercises Salsa queries across mutated workspaces and edit histories.
//!
//! Each selected query must complete without panicking or hanging. Recovery
//! firings provide context but do not identify Salsa's repeated key. The generic
//! runner does not compare query results with expected values.
//!
//! See `crates/oak_db/fuzz/README.md` for commands, CI budgets, corpus
//! maintenance, and failure replay. This module describes query coverage.
//!
//! [`seed_corpus()`] supplies fixed starting scenarios, and [`ScenarioMutator`]
//! mutates them. The test suite replays shrunken failures so the trace, panic
//! location, and artifact describe the same scenario. Save it as an explicit
//! [`Scenario`] test.
//!
//! # Coverage
//!
//! [`Query`] covers 18 of the 50 tracked queries in the Salsa inventory. It
//! includes the production roots `diagnostics()`, `imports()`, `imports_at()`,
//! `resolve_at()`, `resolve()`, `used_packages()`, and `sourced_by()`, all five
//! workspace aggregates, and cold entry into the six file-keyed queries with
//! `cycle_result` handlers.
//!
//! Mutation reaches every `EffectRecipe` variant, both invocation forms, and
//! all three `SourceProvider` variants, including an effect escaped through a
//! `bquote()` hole.
//!
//! `Package::resolve()` is excluded because these workspaces have no NAMESPACE
//! re-exports. Testthat and shiny layouts, file renaming, and metadata or
//! revision edits stay out of reach. Queries outside [`Query`] are covered only
//! as dependencies, not as entry points.

mod artifact;
mod build;
mod choose;
pub mod corpus;
mod generate;
mod mutate;
mod panics;
mod run;
mod scenario;
mod spec;

pub use generate::seed_corpus;
pub use mutate::ScenarioMutator;
// Keep direct `World` access for regression assertions. External callers run
// whole scenarios so extra queries cannot warm the database before the cold entry.
#[cfg(test)]
pub(crate) use run::start;
pub use run::Runner;
#[cfg(test)]
pub(crate) use run::World;
pub use scenario::Edit;
pub use scenario::Op;
pub use scenario::Query;
pub use scenario::Scenario;
pub use scenario::Site;
pub use spec::FileId;
pub use spec::FileSpec;
pub use spec::Owner;
pub use spec::WorkspaceSpec;

pub use crate::file_imports::CollationView;
