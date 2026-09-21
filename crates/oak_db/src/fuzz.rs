//! Exercises Salsa queries across mutated workspaces and edit histories.
//!
//! Each selected query must complete without panicking or hanging. Recovery
//! firings provide context but do not identify Salsa's repeated key. The generic
//! runner does not compare query results with expected values.
//!
//! See `crates/oak_db/fuzz/README.md` for commands, CI budgets, corpus
//! maintenance, and failure replay. This module describes query coverage.
//!
//! `seed_corpus()` supplies fixed starting scenarios, and `ScenarioMutator`
//! mutates them. The test suite replays shrunken failures so the trace, panic
//! location, and artifact describe the same scenario. Save it as an explicit
//! `Scenario` test.
//!
//! # Coverage
//!
//! `Query` covers 19 of the 50 tracked queries in the Salsa inventory, the ones
//! that are useful as starting poings. It includes production roots such as
//! `diagnostics()`, `imports()`, `imports_at()`, `resolve_at()`, `resolve()`,
//! `used_packages()`, and `sourced_by()`, all five workspace aggregates, and
//! cold entry into the seven queries with `cycle_result` handlers, including
//! `Package::resolve()`.
//!
//! Mutation reaches every `EffectRecipe` variant, both invocation forms, and
//! all three `SourceProvider` variants, including an effect escaped through a
//! `bquote()` hole. Renaming changes one definition or use at a time, including
//! names declared by `NAMESPACE`, so an export or re-export chain can become
//! unresolved. It also reaches `Package::resolve()` both directly through
//! `Query::PackageResolve` and indirectly through a consumer's `library()`
//! attach or a package's own `importFrom`, across acyclic chains, mutual
//! re-export cycles, and effect-name shadowing through `package_binding()`.
//!
//! Each seed corpus contains a re-export cycle with a matching cold entry.
//! `seed_corpus()` assigns package layers by motif position so unrelated changes
//! to random draws cannot remove that coverage.
//!
//! Package-owned `R/*.R` files form the loadable collation, while its remaining
//! files are standalone scripts. A motif-position layout rotation ensures every
//! seed corpus contains a `tests/testthat/` workspace whose test files testthat
//! loads after the package collation, and a Shiny app whose `app.R` marker
//! toggle moves its `R/` siblings between autoload members and plain scripts.
//! An independent rotation covers Shiny-specific detection: disabled `R/`
//! autoload, paired `ui.R` and `server.R` entries, package `inst/app/`, and a
//! nested `R/app.R`. A third rotation covers testthat support ordering and
//! exclusions: `setup*.R`, `teardown*.R`, and nested files.
//!
//! Library packages are metadata-only. Library-owned sources, `import()` bulk
//! imports, `Collate:` ordering, `R/` files excluded from it, file renaming,
//! and metadata or revision edits are excluded.
//!
//! Only `Query` variants are cold entries. Layout queries such as
//! `collation_siblings()`, `source_dir_scripts()`, and `shiny_autoload()` accept
//! `&dyn SourceDb`, so they cannot call a recursive semantic query or become a
//! Salsa repeated key. Add one as an entry only if it declares or reaches a
//! `cycle_result` handler.
//!
//! [`classify_in_package()`]: crate::classify_in_package

mod artifact;
mod budgets;
mod build;
mod choose;
#[cfg(test)]
pub(crate) mod corpus;
mod driver;
mod generate;
mod limits;
mod mutate;
mod panics;
mod run;
mod scenario;
mod spec;
mod targets;
mod traversal;

pub use driver::execute_json;
pub use driver::mutate_json;
#[cfg(test)]
pub(crate) use generate::seed_corpus;
#[cfg(test)]
pub(crate) use mutate::ScenarioMutator;
// Keep direct `World` access for regression assertions. External callers run
// whole scenarios so extra queries cannot warm the database before the cold entry.
#[cfg(test)]
pub(crate) use run::start;
pub use run::Runner;
#[cfg(test)]
pub(crate) use run::World;
#[cfg(test)]
pub(crate) use scenario::Scenario;
#[cfg(test)]
pub(crate) use spec::FileId;
#[cfg(test)]
pub(crate) use spec::PackageId;
#[cfg(test)]
pub(crate) use spec::WorkspaceSpec;
