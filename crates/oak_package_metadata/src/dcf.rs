use std::collections::HashMap;

/// Parsed DCF file (Debian Control File, e.g. DESCRIPTION). Simple wrapper
/// around the map of fields whose `get()` method returns a `&str` that's easier
/// to work with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dcf {
    pub fields: HashMap<String, String>,
}

impl Dcf {
    pub fn new() -> Self {
        Dcf {
            fields: HashMap::new(),
        }
    }

    pub fn parse(input: &str) -> Self {
        Dcf {
            fields: parse_dcf(input),
        }
    }

    /// Get a field value by key
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(|s| s.as_str())
    }
}

/// Parse a DCF (Debian Control File) format string into a key-value map.
/// https://www.debian.org/doc/debian-policy/ch-controlfields.html
fn parse_dcf(input: &str) -> std::collections::HashMap<String, String> {
    use std::collections::HashMap;

    let mut fields = HashMap::new();
    let mut current_key: Option<String> = None;
    let mut current_value = String::new();

    for line in input.lines() {
        // Indented line: This is a continuation, even if empty
        if line.starts_with(char::is_whitespace) {
            current_value.push_str(line);
            current_value.push('\n');
            continue;
        }

        // Non-whitespace at start and contains a colon: This is a new field
        if !line.is_empty() && line.contains(':') {
            // Save previous field
            if let Some(key) = current_key.take() {
                fields.insert(key, current_value.trim_end().to_string());
            }

            let idx = line.find(':').unwrap();
            let key = line[..idx].trim().to_string();
            let value = line[idx + 1..].trim_start();

            current_key = Some(key);

            current_value.clear();
            current_value.push_str(value);
            current_value.push('\n');

            continue;
        }
    }

    // Finish last field
    if let Some(key) = current_key {
        fields.insert(key, current_value.trim_end().to_string());
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dcf_basic() {
        let dcf = r#"Package: mypackage
Version: 1.0.0
Title: My Package
Description: A simple package for testing."#;
        let parsed = Dcf::parse(dcf);
        assert_eq!(parsed.get("Package"), Some("mypackage"));
        assert_eq!(parsed.get("Version"), Some("1.0.0"));
        assert_eq!(parsed.get("Title"), Some("My Package"));
        assert_eq!(
            parsed.get("Description"),
            Some("A simple package for testing.")
        );
    }

    #[test]
    fn parses_dcf_multiline_field() {
        let dcf = r#"Package: mypackage
Version: 1.0.0
Description: This is a long description
 that spans multiple lines
 and should be preserved correctly."#;
        let parsed = Dcf::parse(dcf);
        assert_eq!(
            parsed.get("Description"),
            Some("This is a long description\n that spans multiple lines\n and should be preserved correctly.")
        );
    }

    // Empty lines are ignored in DCF files. They are supported via a dot
    // notation (` .` represents an empty line) bug we don't support that.
    #[test]
    fn parses_dcf_empty_continuation_line() {
        let dcf = r#"Package: mypackage
Description: First line
 second line

 third line"#;
        let parsed = Dcf::parse(dcf);
        assert_eq!(
            parsed.get("Description"),
            Some("First line\n second line\n third line")
        );
    }
}
