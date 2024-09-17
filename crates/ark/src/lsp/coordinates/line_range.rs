/// Type alias around `TextRange`
///
/// Use this to signal the intent of storing line numbers
/// while still having access to the `TextRange` methods.
pub type LineRange = text_size::TextRange;

/// Constructor from `u32`
pub fn line_range(start: u32, end: u32) -> LineRange {
    LineRange::new(start.into(), end.into())
}
