//
// package_description.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//

use std::collections::HashMap;

use anyhow;

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

/// Parsed DESCRIPTION file
#[derive(Clone, Debug)]
pub struct Description {
    pub name: String,
    pub version: String,

    /// `Depends` field. Currently doesn't contain versions.
    pub depends: Vec<String>,

    /// Raw DCF fields
    pub fields: Dcf,
}

impl Default for Description {
    fn default() -> Self {
        Description {
            name: String::new(),
            version: String::new(),
            depends: Vec::new(),
            fields: Dcf::default(),
        }
    }
}

impl Description {
    /// Parse a DESCRIPTION file in DCF format
    pub fn parse(contents: &str) -> anyhow::Result<Self> {
        let fields = Dcf::parse(contents);

        let name = fields
            .get("Package")
            .ok_or_else(|| anyhow::anyhow!("Missing Package field in DESCRIPTION"))?
            .to_string();

        let version = fields
            .get("Version")
            .ok_or_else(|| anyhow::anyhow!("Missing Version field in DESCRIPTION"))?
            .to_string();

        let depends = fields
            .get("Depends")
            .and_then(|deps| {
                let mut pkgs = parse_comma_separated(deps);

                // Remove dependency on R. In the future we will record it to a field with
                // the minimum version the package depends on.
                pkgs.retain(|pkg| pkg != "R");

                Some(pkgs)
            })
            .unwrap_or_default();

        Ok(Description {
            name,
            version,
            depends,
            fields,
        })
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

/// Parse a comma-separated list of package dependencies
fn parse_comma_separated(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            // Remove version constraints like "R (>= 3.5.0)"
            if let Some(idx) = s.find('(') {
                s[..idx].trim().to_string()
            } else {
                s.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_description_basic() {
        let desc = r#"Package: mypackage
Version: 1.0.0
Title: My Package
Description: A simple package for testing."#;
        let parsed = Description::parse(desc).unwrap();
        assert_eq!(parsed.name, "mypackage");
        assert_eq!(parsed.version, "1.0.0");
        assert!(parsed.depends.is_empty());
    }

    #[test]
    fn parses_description_with_depends() {
        let desc = r#"Package: mypackage
Version: 1.0.0
Depends: R (>= 3.5.0), utils, stats
Title: My Package"#;
        let parsed = Description::parse(desc).unwrap();
        assert_eq!(parsed.name, "mypackage");
        assert_eq!(parsed.version, "1.0.0");
        assert_eq!(parsed.depends, vec!["utils", "stats"]);
    }

    #[test]
    fn parses_description_with_multiline_field() {
        let desc = r#"Package: mypackage
Version: 1.0.0
Description: This is a long description
 that spans multiple lines
 and should be preserved correctly."#;
        let parsed = Description::parse(desc).unwrap();
        assert_eq!(parsed.name, "mypackage");
        assert_eq!(parsed.version, "1.0.0");
    }

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
