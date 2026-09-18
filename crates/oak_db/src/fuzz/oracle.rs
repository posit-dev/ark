//! Checks incremental name resolution against a fresh database over generated
//! histories.
//!
//! Unlike [`crate::fuzz::run`], which detects panics and hangs, this campaign
//! detects stale answers at scheduled resolution checkpoints. It compares
//! results only until either execution recovers, because recovery may return a
//! memoized fallback instead of the current answer. After recovery, it still
//! detects panics and hangs but skips comparisons.
//!
//! Agreement does not prove correctness, because both databases can return the
//! same wrong answer. Named fixtures validate expected values directly.

pub(crate) mod campaign;
pub(crate) mod compare;
pub(crate) mod observe;
mod reduce;
