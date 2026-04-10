use std::fmt;
use std::marker::PhantomData;
use std::ops;

pub trait Idx: Copy + fmt::Debug + Eq {
    fn new(value: usize) -> Self;
    fn index(self) -> usize;
}

/// A `Vec<V>` indexed by a strongly-typed newtype `I` instead of `usize`,
/// so that indices from different vectors can't be mixed up.
pub struct IndexVec<I: Idx, V> {
    raw: Vec<V>,
    _phantom: PhantomData<I>,
}

impl<I: Idx, V> IndexVec<I, V> {
    pub fn new() -> Self {
        Self {
            raw: Vec::new(),
            _phantom: PhantomData,
        }
    }

    pub fn push(&mut self, value: V) -> I {
        let id = self.next_id();
        self.raw.push(value);
        id
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    pub fn next_id(&self) -> I {
        I::new(self.raw.len())
    }

    pub fn iter(&self) -> impl Iterator<Item = (I, &V)> {
        self.raw.iter().enumerate().map(|(i, v)| (I::new(i), v))
    }
}

impl<I: Idx, V> IntoIterator for IndexVec<I, V> {
    type Item = V;
    type IntoIter = std::vec::IntoIter<V>;

    fn into_iter(self) -> Self::IntoIter {
        self.raw.into_iter()
    }
}

impl<I: Idx, V> FromIterator<V> for IndexVec<I, V> {
    fn from_iter<T: IntoIterator<Item = V>>(iter: T) -> Self {
        Self {
            raw: iter.into_iter().collect(),
            _phantom: PhantomData,
        }
    }
}

impl<I: Idx, V> Default for IndexVec<I, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Idx, V: fmt::Debug> fmt::Debug for IndexVec<I, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.raw.iter()).finish()
    }
}

impl<I: Idx, V> ops::Index<I> for IndexVec<I, V> {
    type Output = V;

    fn index(&self, id: I) -> &V {
        &self.raw[id.index()]
    }
}

impl<I: Idx, V> ops::IndexMut<I> for IndexVec<I, V> {
    fn index_mut(&mut self, id: I) -> &mut V {
        &mut self.raw[id.index()]
    }
}

macro_rules! define_index {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            const MAX: usize = u32::MAX as usize - 1;
        }

        impl From<u32> for $name {
            fn from(raw: u32) -> Self {
                Self(raw)
            }
        }

        impl $crate::index_vec::Idx for $name {
            fn new(value: usize) -> Self {
                assert!(value <= Self::MAX);
                Self(value as u32)
            }

            fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

pub(crate) use define_index;
