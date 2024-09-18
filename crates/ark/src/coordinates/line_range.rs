use std::cmp;

/// Range over lines of text
///
/// The API is (incompletely) modeled after `text_size::TextRange` but the inner
/// types are not as opaque so it's easy to work with line numbers coming from
/// external sources. The type name mentions "line" to be self-documenting about
/// what the range represents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LineRange {
    start: u32,
    end: u32,
}

/// Mirrors `text_size::TextRange` API (incompletely)
impl LineRange {
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub fn start(&self) -> u32 {
        self.start
    }

    pub fn end(&self) -> u32 {
        self.end
    }

    pub fn contains(&self, line: u32) -> bool {
        self.start <= line && line < self.end
    }

    pub fn cover(&self, other: Self) -> Self {
        let start = cmp::min(self.start, other.start);
        let end = cmp::max(self.end, other.end);
        Self::new(start, end)
    }
}

impl From<harp::srcref::SrcRef> for LineRange {
    fn from(value: harp::srcref::SrcRef) -> Self {
        Self::new(value.line.start, value.line.end)
    }
}
