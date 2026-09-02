//
// strings.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

/// Split strings in lines
///
/// Same as `str::lines()` but preserves trailing newlines.
///
/// Returns a `DoubleEndedIterator`, which is the same as the
/// one returned by `split()` in this particular case.
pub fn lines(text: &str) -> impl DoubleEndedIterator<Item = &str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
}

#[cfg(test)]
mod tests {
    use crate::strings::lines;

    #[test]
    fn test_lines() {
        let lines: Vec<&str> = lines("foo\n\n\nbar\n\n").collect();
        assert_eq!(lines, vec!["foo", "", "", "bar", "", ""])
    }

    #[test]
    fn test_lines_crlf() {
        let lines: Vec<&str> = lines("foo\r\n\r\nbar\r\n").collect();
        assert_eq!(lines, vec!["foo", "", "bar", ""])
    }
}
