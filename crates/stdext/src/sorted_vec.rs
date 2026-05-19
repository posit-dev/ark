/// A sorted, deduplicated `Vec<T>`. Provides O(log n) lookup via binary search.
/// Derefs to `[T]` for iteration and other slice operations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SortedVec<T> {
    inner: Vec<T>,
}

impl<T: Ord> SortedVec<T> {
    pub fn from_vec(mut values: Vec<T>) -> Self {
        values.sort();
        values.dedup();
        Self { inner: values }
    }

    pub fn into_vec(self) -> Vec<T> {
        self.inner
    }

    pub fn contains(&self, value: &T) -> bool {
        self.inner.binary_search(value).is_ok()
    }
}

impl SortedVec<String> {
    pub fn contains_str(&self, value: &str) -> bool {
        self.inner
            .binary_search_by(|s| s.as_str().cmp(value))
            .is_ok()
    }
}

impl<T> std::ops::Deref for SortedVec<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        &self.inner
    }
}

impl<T> IntoIterator for SortedVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a SortedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.inner.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sorts_and_deduplicates() {
        let sv = SortedVec::from_vec(vec![3, 1, 2, 1, 3]);
        assert_eq!(&*sv, &[1, 2, 3]);
    }

    #[test]
    fn test_contains() {
        let sv = SortedVec::from_vec(vec![10, 30, 20]);
        assert!(sv.contains(&10));
        assert!(sv.contains(&20));
        assert!(sv.contains(&30));
        assert!(!sv.contains(&15));
    }

    #[test]
    fn test_contains_str() {
        let sv = SortedVec::from_vec(vec!["b".to_string(), "a".to_string()]);
        assert!(sv.contains_str("a"));
        assert!(sv.contains_str("b"));
        assert!(!sv.contains_str("c"));
    }

    #[test]
    fn test_empty() {
        let sv = SortedVec::<i32>::from_vec(vec![]);
        assert!(sv.is_empty());
        assert!(!sv.contains(&0));
    }

    #[test]
    fn test_deref_iteration() {
        let sv = SortedVec::from_vec(vec![3, 1, 2]);
        let collected: Vec<_> = sv.iter().copied().collect();
        assert_eq!(collected, vec![1, 2, 3]);
    }
}
