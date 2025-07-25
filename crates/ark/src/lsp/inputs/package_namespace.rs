//
// package_namespace.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//

use tree_sitter::Parser;

use crate::treesitter::TsQuery;

/// Parsed NAMESPACE file
#[derive(Default, Clone, Debug)]
pub struct Namespace {
    /// Names of objects exported with `export()`
    pub exports: Vec<String>,
    /// Names of objects imported with `importFrom()`
    pub imports: Vec<String>,
    /// Names of packages bulk-imported with `import()`
    pub package_imports: Vec<String>,
}

impl Namespace {
    /// Parse a NAMESPACE file using tree-sitter to extract exports and imports.
    pub fn parse(contents: &str) -> anyhow::Result<Self> {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .map_err(|err| anyhow::anyhow!("Failed to set tree-sitter language: {err:?}"))?;

        let tree = parser
            .parse(contents, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse NAMESPACE file"))?;
        let root_node = tree.root_node();

        let query_str = r#"
            (call
                function: (identifier) @fn_name
                arguments: (arguments (argument value: (identifier) @exported))
                (#eq? @fn_name "export")
            )
            (call
                function: (identifier) @fn_name
                arguments: (arguments (argument value: (identifier) @pkg) (argument value: (identifier) @imported))
                (#eq? @fn_name "importFrom")
            )
            (call
                function: (identifier) @fn_name
                arguments: (arguments (argument value: (identifier) @bulk_imported))
                (#eq? @fn_name "import")
            )
        "#;
        let mut ts_query = TsQuery::new(query_str)?;

        let all_captures = ts_query.all_captures(root_node, contents.as_bytes());

        let filter_captures = |capture_name: &str| -> Vec<String> {
            all_captures
                .iter()
                .filter(|(name, _)| name == capture_name)
                .map(|(_, node)| {
                    node.utf8_text(contents.as_bytes())
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        };

        let mut exports = filter_captures("exported");
        let mut imports = filter_captures("imported");
        let mut package_imports = filter_captures("bulk_imported");

        // Take unique values of imports and exports. In the future we'll lint
        // this but for now just be defensive.
        exports.sort();
        exports.dedup();
        imports.sort();
        imports.dedup();
        package_imports.sort();
        package_imports.dedup();

        Ok(Namespace {
            imports,
            exports,
            package_imports,
        })
    }

    /// TODO: Take a `Library` and incorporate bulk imports
    pub(crate) fn _resolve_imports(&self) -> &Vec<String> {
        &self.imports
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exports() {
        let ns = r#"
            export(foo)
            export(bar)
            exports(baz) # typo
        "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.exports, vec!["bar", "foo"]);
        assert!(parsed.imports.is_empty());
    }

    #[test]
    fn parses_importfrom() {
        let ns = r#"
            importFrom(stats, median)
            importFrom(utils, head)
            importsFrom(utils, tail) # typo
        "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.imports, vec!["head", "median"]);
        assert!(parsed.exports.is_empty());
    }

    #[test]
    fn parses_mixed_namespace_with_duplicates() {
        let ns = r#"
            export(foo)
            importFrom(stats, median)
            export(bar)
            importFrom(utils, head)
            importFrom(utils, median)
        "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.exports, vec!["bar", "foo"]);
        assert_eq!(parsed.imports, vec!["head", "median"]);
    }

    #[test]
    fn parses_bulk_imports() {
        let ns = r#"
                import(rlang)
                import(utils)
                export(foo)
                import(utils)
                importFrom(stats, median)
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.package_imports, vec!["rlang", "utils"]);
        assert_eq!(parsed.exports, vec!["foo"]);
        assert_eq!(parsed.imports, vec!["median"]);
    }
}
