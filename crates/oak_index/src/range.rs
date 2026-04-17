use biome_rowan::TextRange;
use biome_rowan::TextSize;

use crate::index_vec::Idx;
use crate::index_vec::IndexVec;

pub trait Ranged {
    fn range(&self) -> TextRange;
}

impl<I: Idx, V: Ranged> IndexVec<I, V> {
    /// Find the `V` containing `offset`, if any.
    pub fn contains(&self, offset: TextSize) -> Option<(I, &V)> {
        self.iter()
            .find(|(_index, value)| value.range().contains(offset))
    }
}
