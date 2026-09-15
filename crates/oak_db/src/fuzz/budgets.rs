//! Budgets for generating and growing scenarios.
//!
//! Keep these within `limits.rs`'s independent artifact acceptance limits.
//! Small graphs permit cycles with a consumer while keeping each check cheap.

// == Resource budgets ==

pub(super) const MAX_FILES: usize = 5;
/// Stop growth at this count; a compound insertion may overshoot it.
pub(super) const MAX_STATEMENTS: usize = 14;
/// A top-level statement plus two nested statement levels.
pub(super) const MAX_DEPTH: usize = 3;
/// Accommodate the seed corpus's three rounds of five operations.
pub(super) const MAX_OPS: usize = 16;
/// Stop growth at this rendered byte count; one insertion may overshoot it.
pub(super) const MAX_TEXT: usize = 2_000;
pub(super) const MAX_PACKAGES: usize = 3;
pub(super) const MAX_EXPORTS: usize = 3;
pub(super) const MAX_REEXPORTS: usize = 3;
