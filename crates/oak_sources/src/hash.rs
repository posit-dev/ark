use sha2::Digest;
use sha2::Sha256;

/// Retain 8 ASCII characters for each hash fragment
pub(crate) fn hash(contents: &str) -> String {
    let mut hash = hex::encode(Sha256::digest(contents));
    hash.truncate(8);
    hash
}
