//
// package_index.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//
//

use std::path::Path;

/// This represents an INDEX file.
///
/// We use it to complement the list of exported symbols in NAMESPACE, in
/// particular for exported datasets. This is a stopgap approach that has known
/// shortcomings (false negatives as we will treat actually non-exported symbols
/// as exported in some cases).
#[derive(Default, Clone, Debug)]
pub struct Index {
    pub names: Vec<String>,
}

impl Index {
    pub fn load_from_folder(path: &Path) -> anyhow::Result<Self> {
        if !path.is_dir() {
            return Err(anyhow::anyhow!(
                "Can't load index as '{path}' is not a folder",
                path = path.to_string_lossy()
            ));
        }

        let index_path = path.join("INDEX");
        if !index_path.is_file() {
            return Err(anyhow::anyhow!(
                "Can't load index as '{path}' does not contain an INDEX file",
                path = path.to_string_lossy()
            ));
        }

        let contents = std::fs::read_to_string(&index_path)?;
        Ok(Index::parse(&contents))
    }

    /// Parses a package index text, extracting valid R symbol names from the first column.
    /// Only names starting at the beginning of a line and consisting of letters, digits, dots, or underscores are included.
    pub fn parse(input: &str) -> Self {
        let valid_name = regex::Regex::new(r"^[A-Za-z.][A-Za-z0-9._]*$").unwrap();
        let mut names = Vec::new();

        for line in input.lines() {
            // Only consider lines that start at column 0 (no leading whitespace)
            if line.starts_with(char::is_whitespace) {
                continue;
            }
            if let Some(first_col) = line.split_whitespace().next() {
                if valid_name.is_match(first_col) {
                    names.push(first_col.to_string());
                }
            }
        }

        Index { names }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_index_parses_simple_names() {
        let input = "\
foo     Description of foo
bar     Description of bar
baz     Description of baz
";
        let idx = Index::parse(input);
        assert_eq!(idx.names, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn test_index_ignores_continuation_lines() {
        let input = "\
foo     Description of foo
        Continuation of description
bar     Description of bar
";
        let idx = Index::parse(input);
        assert_eq!(idx.names, vec!["foo", "bar"]);
    }

    #[test]
    fn test_index_parses_names_with_dots_and_underscores() {
        let input = "\
foo.bar     Description
foo_bar     Description
.foo        Description
";
        let idx = Index::parse(input);
        assert_eq!(idx.names, vec!["foo.bar", "foo_bar", ".foo"]);
    }

    #[test]
    fn test_index_skips_names_with_dashes() {
        let input = "\
foo-bar     Description
baz         Description
foo         Description
";
        let idx = Index::parse(input);
        assert_eq!(idx.names, vec!["baz", "foo"]);
    }

    #[test]
    fn test_index_skips_lines_without_valid_names() {
        let input = "\
123foo      Not a valid name
_bar        Not a valid name
.foo        Valid name
";
        let idx = Index::parse(input);
        assert_eq!(idx.names, vec![".foo"]);
    }

    #[test]
    fn test_index_parses_realistic_package_index() {
        let input = "\
.prt.methTit            Print and Summary Method Utilities for Mixed
                        Effects
Arabidopsis             Arabidopsis clipping/fertilization data
Dyestuff                Yield of dyestuff by batch
GHrule                  Univariate Gauss-Hermite quadrature rule
NelderMead-class        Class '\"NelderMead\"' of Nelder-Mead optimizers
                        and its Generator
Nelder_Mead             Nelder-Mead Optimization of Parameters,
                        Possibly (Box) Constrained
";
        let idx = Index::parse(input);
        assert_eq!(idx.names, vec![
            ".prt.methTit",
            "Arabidopsis",
            "Dyestuff",
            "GHrule",
            "Nelder_Mead"
        ]);
    }

    #[test]
    fn load_from_folder_returns_errors() {
        // From a file
        let file = tempfile::NamedTempFile::new().unwrap();
        let result = Index::load_from_folder(file.path());
        assert!(result.is_err());

        // From a dir without an INDEX file
        let dir = tempdir().unwrap();
        let result = Index::load_from_folder(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn load_from_folder_reads_and_parses_index() {
        let dir = tempdir().unwrap();
        let index_path = dir.path().join("INDEX");
        let content = "\
foo     Description of foo
bar     Description of bar
";
        fs::write(&index_path, content).unwrap();

        let idx = Index::load_from_folder(dir.path()).unwrap();
        assert_eq!(idx.names, vec!["foo", "bar"]);
    }
}
