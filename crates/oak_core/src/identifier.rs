//! R identifier syntax and reserved words.
//!
//! Language-level facts about what counts as a valid R identifier. Used
//! by rename, diagnostics, completions, and anything else that needs to
//! emit or recognise identifier text.

use anyhow::anyhow;

/// A validated R name with semantic and source forms.
///
/// Parsing once keeps string-form and identifier sites consistent.
#[derive(Debug)]
pub struct RName {
    semantic: String,
    identifier: String,
}

impl RName {
    /// Parse a bare or backtick-wrapped R name.
    ///
    /// Wrapped reserved words are valid. Empty names, bare reserved words, and
    /// stray backticks return `Err`.
    pub fn parse(name: &str) -> anyhow::Result<Self> {
        if name.is_empty() {
            return Err(anyhow!("Identifier cannot be empty"));
        }

        if let Some(inner) = name
            .strip_prefix('`')
            .and_then(|rest| rest.strip_suffix('`'))
        {
            return Self::from_wrapped(inner);
        }
        if name.contains('`') {
            return Err(anyhow!("Identifier cannot contain a backtick"));
        }
        if is_reserved(name) {
            return Err(anyhow!("`{name}` is a reserved word in R"));
        }

        Ok(Self::from_semantic(name))
    }

    /// The source spelling for this binding, backtick-wrapped when necessary.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// Render the semantic name as an R string literal using `delimiter`.
    ///
    /// String-form binding sites store the semantic name, not its backticked
    /// source spelling.
    pub fn quoted(&self, delimiter: char) -> String {
        let escaped = self
            .semantic
            .replace('\\', "\\\\")
            .replace(delimiter, &format!("\\{delimiter}"));
        format!("{delimiter}{escaped}{delimiter}")
    }

    fn from_wrapped(inner: &str) -> anyhow::Result<Self> {
        if inner.is_empty() {
            return Err(anyhow!("Identifier cannot be empty"));
        }
        if inner.contains('`') {
            return Err(anyhow!("Identifier cannot contain a backtick"));
        }
        if is_reserved(inner) {
            return Ok(Self {
                semantic: inner.to_string(),
                identifier: format!("`{inner}`"),
            });
        }

        Ok(Self::from_semantic(inner))
    }

    fn from_semantic(name: &str) -> Self {
        let identifier = if is_valid_identifier(name) {
            name.to_string()
        } else {
            format!("`{name}`")
        };
        Self {
            semantic: name.to_string(),
            identifier,
        }
    }
}

/// Whether `name` is a valid bare R identifier (no backticks needed).
///
/// R's rule: starts with a letter or `.`, then letters, digits, `.`, or
/// `_`. "Letter" is Unicode-aware (matching `iswalpha` in R's UTF-8
/// locale), so non-ASCII identifiers like `μ`, `αβ`, and `文字` are valid.
/// Digits are ASCII-only (per `?make.names`: "only ASCII digits are
/// considered to be digits"). A leading `.` followed by an ASCII digit
/// is a number literal, not an identifier (e.g. `.5`).
pub fn is_valid_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_alphabetic() || first == '.') {
        return false;
    }
    if first == '.' {
        if let Some(second) = name.chars().nth(1) {
            if second.is_ascii_digit() {
                return false;
            }
        }
    }
    chars.all(|c| c.is_alphabetic() || c.is_ascii_digit() || c == '.' || c == '_')
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
    fn test_non_ascii_identifiers() {
        // Unicode letters are valid (R's `iswalpha` in UTF-8 locale).
        assert!(is_valid_identifier("μ"));
        assert!(is_valid_identifier("αβ"));
        assert!(is_valid_identifier("文字"));
        // Mixed with ASCII and continuation chars.
        assert!(is_valid_identifier("foo_μ"));
        assert!(is_valid_identifier("μ2"));
        assert!(is_valid_identifier(".μ"));
        assert_eq!(identifier_text("μ"), "μ");
        assert_eq!(identifier_text("αβ"), "αβ");
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

    fn identifier_text(name: &str) -> String {
        RName::parse(name).unwrap().identifier().to_string()
    }

    #[test]
    fn test_rname_plain() {
        assert_eq!(identifier_text("foo"), "foo");
    }

    #[test]
    fn test_rname_wraps_non_identifier() {
        assert_eq!(identifier_text("foo bar"), "`foo bar`");
        assert_eq!(identifier_text("1foo"), "`1foo`");
    }

    #[test]
    fn test_rname_rejects_empty() {
        assert!(RName::parse("").is_err());
    }

    #[test]
    fn test_rname_rejects_reserved() {
        assert!(RName::parse("if").is_err());
    }

    #[test]
    fn test_rname_rejects_backtick() {
        assert!(RName::parse("foo`bar").is_err());
    }

    #[test]
    fn test_rname_keeps_necessary_backticks() {
        assert_eq!(identifier_text("`foo bar`"), "`foo bar`");
        // Backticks make reserved words valid R identifiers.
        assert_eq!(identifier_text("`if`"), "`if`");
    }

    #[test]
    fn test_rname_strips_unnecessary_backticks() {
        assert_eq!(identifier_text("`bar`"), "bar");
        assert_eq!(identifier_text("`foo.bar`"), "foo.bar");
    }

    #[test]
    fn test_rname_rejects_pre_wrapped_edge_cases() {
        assert!(RName::parse("`").is_err());
        assert!(RName::parse("``").is_err());
        assert!(RName::parse("`foo`bar`").is_err());
    }

    #[test]
    fn test_rname_quoted_wraps_in_delimiter() {
        assert_eq!(RName::parse("bar").unwrap().quoted('"'), "\"bar\"");
        assert_eq!(RName::parse("bar").unwrap().quoted('\''), "'bar'");
    }

    #[test]
    fn test_rname_quoted_drops_backticks() {
        assert_eq!(RName::parse("`bar`").unwrap().quoted('"'), "\"bar\"");
        assert_eq!(
            RName::parse("`foo bar`").unwrap().quoted('"'),
            "\"foo bar\""
        );
        assert_eq!(RName::parse("`if`").unwrap().quoted('"'), "\"if\"");
    }

    #[test]
    fn test_rname_quoted_escapes_delimiter_and_backslash() {
        // Defensive: a real identifier never contains these, but if a name did,
        // it stays inside the string rather than breaking out of it.
        assert_eq!(RName::parse("a\"b").unwrap().quoted('"'), "\"a\\\"b\"");
        assert_eq!(RName::parse("a\\b").unwrap().quoted('"'), "\"a\\\\b\"");
        // The other delimiter isn't escaped: a `'` inside a `"`-string is literal.
        assert_eq!(RName::parse("a'b").unwrap().quoted('"'), "\"a'b\"");
    }
}
