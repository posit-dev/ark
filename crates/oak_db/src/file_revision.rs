/// A number representing the revision of a file.
///
/// Two revisions that don't compare equal signify that the file has been
/// modified. Revisions aren't guaranteed to be monotonically increasing or in
/// any specific order.
///
/// `oak_scan` derives it from the file's last-modification time (see its
/// `file_revision` helper), but the type is agnostic: any scheme that produces
/// a distinct `u128` per content version works (mtime, content hash, an
/// LSP-provided version). [`crate::File::source_text`] only reads it to decide
/// whether to re-read from disk, it never inspects the value beyond equality.
///
/// Layout follows ty's `FileRevision`: whole seconds in the high 64 bits,
/// sub-second nanos in the low bits (`(seconds << 64) | nanos`). `Hash` is
/// derived because this is a salsa input field, which ty's version doesn't
/// need.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileRevision(u128);

impl FileRevision {
    /// Default revision for files with no meaningful mtime: editor buffers and
    /// untitled docs, plus disk files whose metadata couldn't be read. The
    /// value is never inspected, it only needs to compare unequal once a real
    /// mtime lands so `source_text` re-reads.
    pub const fn zero() -> Self {
        Self(0)
    }
}

impl From<u128> for FileRevision {
    fn from(value: u128) -> Self {
        FileRevision(value)
    }
}

/// Report an untracked read when `revision` is the zero sentinel.
///
/// A query that reads a revision input already records the normal salsa
/// dependency, so a real mtime bump re-runs it. Zero is different: it means we
/// never got a trustworthy mtime, e.g. a transient stat failure on a network
/// drive. Nothing guarantees the revision will ever move off zero, because a
/// file that recovers without changing produces no watcher event. Reporting an
/// untracked read makes salsa re-run the calling query on the next revision, so
/// it retries the disk read instead of pinning its first cached result forever.
///
/// The retry only persists while the file stays both reachable and at zero. A
/// successful read that returns the same bytes backdates, so nothing downstream
/// re-runs, and a deleted or evicted file leaves the live graph and stops being
/// queried at all.
pub(crate) fn report_untracked_if_zero(db: &dyn crate::Db, revision: FileRevision) {
    if revision == FileRevision::zero() {
        db.report_untracked_read();
    }
}

impl From<filetime::FileTime> for FileRevision {
    fn from(value: filetime::FileTime) -> Self {
        // `seconds()` is i64, `nanoseconds()` is u32.
        // We pack both in a u128.
        let seconds = value.seconds() as u128;
        let seconds = seconds << 64;
        let nanos = u128::from(value.nanoseconds());

        FileRevision(seconds | nanos)
    }
}
