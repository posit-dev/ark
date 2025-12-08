//
// package_namespace.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//

use std::sync::LazyLock;

use tree_sitter::Parser;
use tree_sitter::Query;

use crate::lsp::traits::node::NodeExt;
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

        // TODO: `import(foo, except = c(bar, baz))`
        //
        // Regarding `exportMethods`, see WRE: "Note that exporting methods on a
        // generic in the namespace will also export the generic"
        static NAMESPACE_QUERY: LazyLock<Query> = LazyLock::new(|| {
            let query_str = r#"
                (call
                    function: (identifier) @fn_name
                    arguments: (arguments (argument value: (identifier) @exported))
                    (#match? @fn_name "^(export|exportClasses|exportMethods)$")
                )
                (call
                    function: (identifier) @fn_name
                    arguments: (arguments (argument value: (identifier) @pkg) (argument value: (identifier) @imported))
                    (#eq? @fn_name "importFrom")
                )
                (call
                    function: (identifier) @fn_name
                    arguments: (arguments (argument value: (identifier) @imported_pkgs))
                    (#eq? @fn_name "import")
                )
            "#;
            let language = &tree_sitter_r::LANGUAGE.into();
            tree_sitter::Query::new(language, query_str).expect("Failed to compile NAMESPACE query")
        });

        let mut ts_query = TsQuery::from_query(&*NAMESPACE_QUERY);

        let captures = ts_query.captures_by(
            root_node,
            &["exported", "imported", "imported_pkgs"],
            contents.as_bytes(),
        );

        let as_strings = |nodes: &Vec<tree_sitter::Node>| {
            nodes
                .iter()
                .map(|node| node.node_as_str(&contents).unwrap_or("").to_string())
                .collect::<Vec<_>>()
        };

        let mut exports = captures.get("exported").map(as_strings).unwrap_or_default();
        let mut imports = captures.get("imported").map(as_strings).unwrap_or_default();
        let mut package_imports = captures
            .get("imported_pkgs")
            .map(as_strings)
            .unwrap_or_default();

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

    #[test]
    fn parses_multiple_args() {
        let ns = r#"
                import(foo, bar)
                export(baz, qux)
                importFrom(pkg, a, b, c)
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.imports, vec!["a", "b", "c"]);
        assert_eq!(parsed.package_imports, vec!["bar", "foo"]);
        assert_eq!(parsed.exports, vec!["baz", "qux"]);
    }

    #[test]
    fn parses_s4_exports() {
        let ns = r#"
                exportClasses(foo)
                exportClasses(bar, baz)
                exportMethods(qux)
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.exports, vec!["bar", "baz", "foo", "qux"]);
    }
}
