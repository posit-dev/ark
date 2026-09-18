//! Compares incremental and fresh name resolution over a restricted scenario
//! family.
//!
//! The unrestricted campaign in [`crate::fuzz::run`] detects only panics and
//! hangs, not stale answers. For scenarios accepted by [`eligible::admit()`], a
//! query depends only on the current workspace, so incremental evaluation must
//! match a database rebuilt from that workspace.
//!
//! This comparison is not a proof because both databases can agree on a wrong
//! answer. Named fixtures assert the expected values.

pub(crate) mod compare;
mod eligible;
pub(crate) mod observe;
