//! R identifier syntax and reserved words.
//!
//! Language-level facts about what counts as a valid R identifier. Used
//! by rename, diagnostics, completions, and anything else that needs to
//! emit or recognise identifier text.

use anyhow::anyhow;

/// Convert a semantic name into its canonical R source form.
///
/// A name that parses as a plain R identifier (and isn't a reserved
/// word) returns as-is. Anything else gets wrapped in backticks. Empty
/// names, reserved words, and names containing a literal backtick return
/// `Err` (a backtick can't appear inside a backtick-quoted identifier).
pub fn to_identifier_text(name: &str) -> anyhow::Result<String> {
    if name.is_empty() {
        return Err(anyhow!("Identifier cannot be empty"));
    }
    if is_reserved(name) {
        return Err(anyhow!("`{name}` is a reserved word in R"));
    }
    if is_valid_identifier(name) {
        return Ok(name.to_string());
    }
    if name.contains('`') {
        return Err(anyhow!("Identifier cannot contain a backtick"));
    }
    Ok(format!("`{name}`"))
}

/// Whether `name` is a valid bare R identifier (no backticks needed).
///
/// R's rule: starts with a letter or `.`, then letters, digits, `.`, or
/// `_`. A leading `.` followed by a digit is a number literal, not an
/// identifier (e.g. `.5`).
pub fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '.') {
        return false;
    }
    if first == '.' {
        if let Some(second) = name.chars().nth(1) {
            if second.is_ascii_digit() {
                return false;
            }
        }
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}

/// R reserved words that cannot be used as identifier names. Source:
/// `?Reserved` in R. Note that `return` is a function, not a reserved
/// word, so it's missing from this list (`return <- 1` is valid R).
/// `_` became reserved in R 4.2 for use as the `|>` pipe placeholder.
pub fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "if" | "else" |
            "for" |
            "while" |
            "repeat" |
            "break" |
            "next" |
            "function" |
            "in" |
            "TRUE" |
            "FALSE" |
            "NULL" |
            "NA" |
            "NA_integer_" |
            "NA_real_" |
            "NA_complex_" |
            "NA_character_" |
            "NaN" |
            "Inf" |
            "..." |
            "_"
    ) || is_dot_dot_n(name)
}

/// `..1`, `..2`, ..., the variadic positional accessors. Listed in
/// `?Reserved` as "..1, ..2 etc.".
fn is_dot_dot_n(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("..") else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_identifiers() {
        assert!(is_valid_identifier("foo"));
        assert!(is_valid_identifier(".foo"));
        assert!(is_valid_identifier("foo.bar"));
        assert!(is_valid_identifier("foo_bar"));
        assert!(is_valid_identifier("foo123"));
    }

    #[test]
    fn test_invalid_identifiers() {
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1foo"));
        assert!(!is_valid_identifier("_foo"));
        assert!(!is_valid_identifier(".1foo"));
        assert!(!is_valid_identifier("foo bar"));
        assert!(!is_valid_identifier("foo-bar"));
    }

    #[test]
    fn test_reserved_words() {
        for word in [
            "if", "for", "function", "TRUE", "FALSE", "NULL", "NA", "...", "_",
        ] {
            assert!(is_reserved(word));
        }
        // `return` is a function, not reserved (`return <- 1` is valid R).
        assert!(!is_reserved("return"));
        // `T` / `F` are reassignable aliases for TRUE/FALSE.
        assert!(!is_reserved("T"));
        assert!(!is_reserved("F"));
        assert!(!is_reserved("foo"));
    }

    #[test]
    fn test_dot_dot_n_is_reserved() {
        assert!(is_reserved("..1"));
        assert!(is_reserved("..2"));
        assert!(is_reserved("..42"));
        // `..` alone is just an identifier.
        assert!(!is_reserved(".."));
        // `..foo` is not reserved (variadic accessors require digits).
        assert!(!is_reserved("..foo"));
        // `.1` is a number literal, not even an identifier.
        assert!(!is_reserved(".1"));
    }

    #[test]
    fn test_to_identifier_text_plain() {
        assert_eq!(to_identifier_text("foo").unwrap(), "foo");
    }

    #[test]
    fn test_to_identifier_text_wraps_non_identifier() {
        assert_eq!(to_identifier_text("foo bar").unwrap(), "`foo bar`");
        assert_eq!(to_identifier_text("1foo").unwrap(), "`1foo`");
    }

    #[test]
    fn test_to_identifier_text_rejects_empty() {
        assert!(to_identifier_text("").is_err());
    }

    #[test]
    fn test_to_identifier_text_rejects_reserved() {
        assert!(to_identifier_text("if").is_err());
    }

    #[test]
    fn test_to_identifier_text_rejects_backtick() {
        assert!(to_identifier_text("foo`bar").is_err());
    }
}
