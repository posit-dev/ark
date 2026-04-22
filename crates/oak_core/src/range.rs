use biome_rowan::TextRange;

pub trait Ranged {
    fn range(&self) -> TextRange;
}
