//
// strings.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

/// Split strings in lines
///
/// Same as `str::lines()` but preserves trailing newlines.
pub fn lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| {
            let Some(line) = line.strip_suffix('\n') else {
                return line;
            };
            let Some(line) = line.strip_suffix('\r') else {
                return line;
            };
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::strings::lines;

    #[test]
    fn test_lines() {
        let lines = lines("foo\n\n\nbar\n\n");
        assert_eq!(lines, vec!["foo", "", "", "bar", "", ""])
    }
}
