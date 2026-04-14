use aether_parser::RParserOptions;
use aether_syntax::AnyRExpression;
use aether_syntax::RArgument;
use aether_syntax::RIdentifier;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstSeparatedList;
use biome_rowan::SyntaxResult;

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
    /// Parse a NAMESPACE file to extract exports and imports.
    pub fn parse(contents: &str) -> anyhow::Result<Self> {
        let parsed = aether_parser::parse(contents, RParserOptions::default());

        if let Some(err) = parsed.error() {
            return Err(anyhow::anyhow!("Failed to parse NAMESPACE file: {err:?}"));
        }

        let root = parsed.tree();

        let mut exports = Vec::new();
        let mut imports = Vec::new();
        let mut package_imports = Vec::new();

        for expr in root.expressions().iter() {
            let AnyRExpression::RCall(call) = expr else {
                continue;
            };
            let Ok(AnyRExpression::RIdentifier(fn_ident)) = call.function() else {
                continue;
            };
            let fn_name = identifier_text(&fn_ident);
            let Ok(args) = call.arguments() else {
                continue;
            };

            // TODO: `import(foo, except = c(bar, baz))`
            //
            // Regarding `exportMethods`, see WRE: "Note that exporting methods on a
            // generic in the namespace will also export the generic"
            match fn_name.as_str() {
                "export" | "exportClasses" | "exportMethods" => {
                    collect_arg_identifiers(args.items().iter(), &mut exports);
                },
                "importFrom" => {
                    // First arg is the package name, rest are imported names
                    collect_arg_identifiers(args.items().iter().skip(1), &mut imports);
                },
                "import" => {
                    collect_arg_identifiers(args.items().iter(), &mut package_imports);
                },
                _ => {},
            }
        }

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

/// Collect identifier names from call arguments.
fn collect_arg_identifiers(
    args: impl Iterator<Item = SyntaxResult<RArgument>>,
    out: &mut Vec<String>,
) {
    for item in args {
        let Ok(arg) = item else { continue };
        let Some(AnyRExpression::RIdentifier(ident)) = arg.value() else {
            continue;
        };
        out.push(identifier_text(&ident));
    }
}

/// Extract the text of an `RIdentifier`, stripping backticks if present.
fn identifier_text(ident: &RIdentifier) -> String {
    let text = ident.syntax().text_trimmed().to_string();
    match text.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
        Some(inner) => inner.to_string(),
        None => text,
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
