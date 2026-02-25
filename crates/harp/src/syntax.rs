//
// syntax.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use libr::SEXP;
use once_cell::sync::Lazy;
use regex::Regex;

use crate::object::RObject;

// Regex for syntactic R identifiers (without reserved word or dot-digit checks).
// Matches R's internal rules from gram.y for valid identifier characters.
static RE_SYNTACTIC_IDENTIFIER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[\p{L}\p{Nl}.][\p{L}\p{Nl}\p{Mn}\p{Mc}\p{Nd}\p{Pc}.]*$").unwrap());

// Reserved words from R's gram.y that always need backtick-quoting.
// Sorted in Rust's byte-wise string ordering for binary_search.
const RESERVED_WORDS: &[&str] = &[
    "FALSE",
    "Inf",
    "NA",
    "NA_character_",
    "NA_complex_",
    "NA_integer_",
    "NA_real_",
    "NULL",
    "NaN",
    "TRUE",
    "break",
    "else",
    "for",
    "function",
    "if",
    "in",
    "next",
    "repeat",
    "while",
];

fn is_reserved_word(name: &str) -> bool {
    RESERVED_WORDS.binary_search(&name).is_ok()
}

#[harp::register]
pub extern "C-unwind" fn harp_is_valid_symbol(name: SEXP) -> anyhow::Result<SEXP> {
    let name = String::try_from(RObject::view(name))?;
    let result = is_valid_symbol(&name);
    Ok(unsafe { libr::Rf_ScalarLogical(result as i32) })
}

/// Returns `true` if `name` is a syntactic R identifier that doesn't need backtick-quoting.
///
/// A syntactic identifier:
/// - Is not empty
/// - Is not a reserved word (NULL, NA, TRUE, FALSE, if, for, etc.)
/// - Starts with a letter (Unicode L or Nl category) or `.`
/// - If it starts with `.`, the second character must not be a digit
/// - Contains only letters, digits, `_`, or `.` in subsequent positions
pub fn is_valid_symbol(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    if is_reserved_word(name) {
        return false;
    }

    if !RE_SYNTACTIC_IDENTIFIER.is_match(name) {
        return false;
    }

    // Reject dot-digit sequences like `.0`, `.1e5` which parse as numeric literals
    if name.starts_with('.') {
        if let Some(second) = name.chars().nth(1) {
            if second.is_ascii_digit() {
                return false;
            }
        }
    }

    true
}

pub fn sym_quote_invalid(name: &str) -> String {
    if is_valid_symbol(name) {
        name.to_string()
    } else {
        sym_quote(name)
    }
}

pub fn sym_quote(name: &str) -> String {
    format!("`{}`", name.replace("`", "\\`"))
}

#[cfg(test)]
mod tests {
    use super::is_reserved_word;
    use super::is_valid_symbol;
    use super::RESERVED_WORDS;

    #[test]
    fn test_reserved_words_are_sorted() {
        let mut sorted = RESERVED_WORDS.to_vec();
        sorted.sort();
        assert_eq!(RESERVED_WORDS, sorted.as_slice());
    }

    #[test]
    fn test_is_syntactic_symbol_reserved_words() {
        for word in RESERVED_WORDS {
            assert!(!is_valid_symbol(word));
            assert!(is_reserved_word(word));
        }
    }

    #[test]
    fn test_is_syntactic_symbol_valid_simple() {
        for name in [".", "a", "Z"] {
            assert!(is_valid_symbol(name));
        }
    }

    #[test]
    fn test_is_syntactic_symbol_invalid_start() {
        for name in ["1", ".1", "~", "!"] {
            assert!(!is_valid_symbol(name));
        }
        for name in ["_", "_foo", "1foo"] {
            assert!(!is_valid_symbol(name));
        }
    }

    #[test]
    fn test_is_syntactic_symbol_invalid_chars() {
        for name in [".fo!o", "b&ar", "baz <- _baz", "~quux.", "h~unoz_"] {
            assert!(!is_valid_symbol(name));
        }
    }

    #[test]
    fn test_is_syntactic_symbol_valid() {
        for name in [".foo", "._1", "bar", "baz_baz", "quux.", "hunoz_", "..."] {
            assert!(is_valid_symbol(name));
        }
    }

    #[test]
    fn test_is_syntactic_symbol_empty() {
        assert!(!is_valid_symbol(""));
    }
}
