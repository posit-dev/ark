use crate::dcf::Dcf;

/// Parsed DESCRIPTION file
#[derive(Clone, Debug, Default)]
pub struct Description {
    pub name: String,
    pub version: String,

    /// `Depends` field. Currently doesn't contain versions.
    pub depends: Vec<String>,

    pub repository: Option<Repository>,

    pub priority: Option<Priority>,

    /// Raw DCF fields
    pub fields: Dcf,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Repository {
    CRAN,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Priority {
    Base,
    Recommended,
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
            .map(|deps| {
                let mut pkgs = parse_comma_separated(deps);

                // Remove dependency on R. In the future we will record it to a field with
                // the minimum version the package depends on.
                pkgs.retain(|pkg| pkg != "R");

                pkgs
            })
            .unwrap_or_default();

        let repository = fields.get("Repository").and_then(|repository| {
            if repository == "CRAN" {
                return Some(Repository::CRAN);
            }
            None
        });

        let priority = fields.get("Priority").and_then(|priority| {
            if priority == "base" {
                return Some(Priority::Base);
            }
            if priority == "recommended" {
                return Some(Priority::Recommended);
            }
            None
        });

        Ok(Description {
            name,
            version,
            depends,
            repository,
            priority,
            fields,
        })
    }

    /// Parse the `Collate` field, if present, returning the whitespace-separated
    /// file names in the order specified.
    pub fn collate(&self) -> Option<Vec<String>> {
        let collate = self.fields.get("Collate")?;
        Some(collate.split_whitespace().map(|s| s.to_string()).collect())
    }
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
    fn parses_description_with_known_repository() {
        let desc = r#"Package: mypackage
Version: 1.0.0
Title: My Package
Repository: CRAN"#;
        let parsed = Description::parse(desc).unwrap();
        assert_eq!(parsed.repository, Some(Repository::CRAN));
    }

    #[test]
    fn parses_description_with_priority() {
        let desc = r#"Package: utils
Version: 4.5.0
Priority: base"#;
        let parsed = Description::parse(desc).unwrap();
        assert_eq!(parsed.priority, Some(Priority::Base));

        let desc = r#"Package: MASS
Version: 7.3-65
Priority: recommended"#;
        let parsed = Description::parse(desc).unwrap();
        assert_eq!(parsed.priority, Some(Priority::Recommended));

        let desc = r#"Package: mypkg
Version: 1.0.0"#;
        let parsed = Description::parse(desc).unwrap();
        assert!(parsed.priority.is_none());
    }

    #[test]
    fn parses_description_with_unknown_repository() {
        let desc = r#"Package: mypackage
Version: 1.0.0
Title: My Package
Repository: notCRAN"#;
        let parsed = Description::parse(desc).unwrap();
        assert_eq!(parsed.repository, None);
    }
}
