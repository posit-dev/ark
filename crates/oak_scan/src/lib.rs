//! Scanners and update helpers for [`oak_db`].
//!
//! `oak_db` is read-only: queries that walk the input graph. `oak_scan`
//! is the write side: it walks the filesystem, parses package metadata,
//! and pushes the result into salsa inputs through a small set of named
//! helpers.
//!
//! Identity is preserved across rescans. A `File` is keyed by URL, a
//! `Package` by its `DESCRIPTION` name within its root, and a `Root` by
//! its path. Repeat scans reuse existing entities and update their
//! fields in place, so downstream salsa caches (parse, semantic_index,
//! `Definition` entities for goto-def) stay valid across edits that
//! don't actually touch a given file's content. This matters for
//! workflows like git branch switching, where files routinely
//! disappear and reappear: we keep the same `File` entity instead of
//! minting fresh ones each time. Since Salsa doesn't do garbage collection,
//! recreating files would grow the cache unboundedly.
//!
//! The trade-off is a small placement invariant: `file.package` must
//! agree with which container Vec holds the file (`pkg.files`,
//! `root.scripts`, or `orphan_root().files`). The helpers in this
//! crate are the only intended callers of the placement-affecting
//! setters on `oak_db`'s input structs.

mod inputs;
mod library;
mod packages;
mod stale;

#[cfg(test)]
mod tests;

pub use inputs::DbExt;
pub use inputs::FileEntry;
pub use inputs::RootExt;
