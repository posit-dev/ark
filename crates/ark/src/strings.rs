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
pub fn lines<'a>(text: &'a str) -> impl DoubleEndedIterator<Item = &'a str> {
    text.split('\n').map(|line| {
        let Some(line) = line.strip_suffix('\n') else {
            return line;
        };
        let Some(line) = line.strip_suffix('\r') else {
            return line;
        };
        line
    })
}

#[cfg(test)]
mod tests {
    use crate::strings::lines;

    #[test]
    fn test_lines() {
        let lines: Vec<&str> = lines("foo\n\n\nbar\n\n").collect();
        assert_eq!(lines, vec!["foo", "", "", "bar", "", ""])
    }
}
