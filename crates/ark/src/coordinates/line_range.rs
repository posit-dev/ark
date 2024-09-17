use std::cmp;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LineRange {
    pub inner: std::ops::Range<u32>,
}

/// Mirrors `text_size::TextRange` API (incompletely)
impl LineRange {
    pub fn new(start: u32, end: u32) -> Self {
        Self {
            inner: std::ops::Range { start, end },
        }
    }

    pub fn start(&self) -> u32 {
        self.inner.start
    }

    pub fn end(&self) -> u32 {
        self.inner.end
    }

    pub fn contains(&self, line: u32) -> bool {
        self.inner.contains(&line)
    }

    pub fn cover(&self, other: Self) -> Self {
        let start = cmp::min(self.start(), other.start());
        let end = cmp::max(self.end(), other.end());
        Self::new(start, end)
    }
}

impl From<harp::srcref::SrcRef> for LineRange {
    fn from(value: harp::srcref::SrcRef) -> Self {
        Self::new(value.line.start, value.line.end)
    }
}
