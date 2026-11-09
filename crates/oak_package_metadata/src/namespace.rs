use aether_parser::RParserOptions;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
use aether_syntax::RArgument;
use aether_syntax::RSyntaxKind::R_RAW_STRING_LITERAL;
use biome_rowan::AstNodeList;
use biome_rowan::AstSeparatedList;
use biome_rowan::SyntaxResult;
use oak_core::syntax_ext::AnyRStatementExt;
use oak_core::syntax_ext::RIdentifierExt;
use stdext::SortedVec;

/// Parsed NAMESPACE file
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Namespace {
    /// Names of objects exported with `export()`, and generics exported with
    /// `exportMethods()`. Class names from `exportClasses()` aren't bindings
    /// reachable through `pkg::`, so they're left out.
    pub exports: SortedVec<String>,
    /// Regular expressions from `exportPattern()`, in file order. Expanding
    /// them requires the package's own bindings, which are unavailable until
    /// its sources are indexed. Consumers compile the patterns.
    pub export_patterns: Vec<String>,
    /// Symbols imported with `importFrom()`, with their source package.
    pub imports: Vec<Import>,
    /// Names of packages bulk-imported with `import()`
    pub package_imports: Vec<String>,
}

/// A single `importFrom()` directive: one symbol imported from a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub name: String,
    pub package: String,
}

impl Namespace {
    /// Parse a NAMESPACE file to extract exports and imports.
    pub fn parse(contents: &str) -> anyhow::Result<Self> {
        let parsed = aether_parser::parse(contents, RParserOptions::default());

        if let Some(diagnostic) = parsed.diagnostics().first() {
            return Err(anyhow::anyhow!(
                "Failed to parse NAMESPACE file: {message}",
                message = diagnostic.message
            ));
        }

        let root = parsed.tree();

        let mut exports = Vec::new();
        let mut export_patterns = Vec::new();
        let mut imports = Vec::new();
        let mut package_imports = Vec::new();

        for statement in root.statements().iter() {
            let Some(AnyRExpression::RCall(call)) = statement.expression() else {
                continue;
            };
            let Ok(AnyRExpression::RIdentifier(fn_ident)) = call.function() else {
                continue;
            };
            let fn_name = fn_ident.name_text();
            let Ok(args) = call.arguments() else {
                continue;
            };

            // TODO: `import(foo, except = c(bar, baz))`
            //
            // Regarding `exportMethods`, see WRE: "Note that exporting methods on a
            // generic in the namespace will also export the generic"
            match fn_name.as_str() {
                "export" | "exportMethods" => {
                    collect_arg_identifiers(args.items().iter(), &mut exports);
                },
                "exportPattern" => {
                    collect_patterns(args.items().iter(), &mut export_patterns);
                },
                "importFrom" => {
                    collect_imports(args.items().iter(), &mut imports);
                },
                "import" => {
                    collect_arg_identifiers(args.items().iter(), &mut package_imports);
                },
                _ => {},
            }
        }

        // Take unique values of imports and exports. In the future we'll lint
        // this but for now just be defensive.
        let exports = SortedVec::from_vec(exports);
        imports.sort_by(|a, b| a.name.cmp(&b.name));
        imports.dedup_by(|a, b| a.name == b.name);
        package_imports.sort();
        package_imports.dedup();

        Ok(Namespace {
            imports,
            exports,
            export_patterns,
            package_imports,
        })
    }

    /// TODO: Take a `Library` and incorporate bulk imports
    pub(crate) fn _resolve_imports(&self) -> &Vec<Import> {
        &self.imports
    }
}

/// Collect `importFrom(pkg, a, b, c)` into `Import` entries. The first
/// argument is the package name, the rest are imported symbols.
fn collect_imports(args: impl Iterator<Item = SyntaxResult<RArgument>>, out: &mut Vec<Import>) {
    let mut args = args;
    let Some(Ok(first_arg)) = args.next() else {
        return;
    };
    let Some(pkg_name) = first_arg.value().as_ref().and_then(directive_name) else {
        return;
    };

    for item in args {
        let Ok(arg) = item else { continue };
        let Some(name) = arg.value().as_ref().and_then(directive_name) else {
            continue;
        };
        out.push(Import {
            name,
            package: pkg_name.clone(),
        });
    }
}

/// Collect identifier names from call arguments.
fn collect_arg_identifiers(
    args: impl Iterator<Item = SyntaxResult<RArgument>>,
    out: &mut Vec<String>,
) {
    for item in args {
        let Ok(arg) = item else { continue };
        let Some(name) = arg.value().as_ref().and_then(directive_name) else {
            continue;
        };
        out.push(name);
    }
}

/// Collect `exportPattern()` arguments, decoding the string escapes a regex
/// needs (`"^[^\\.]"` is the regex `^[^\.]`). Raw strings have no escapes, so
/// `r"(^[^\.])"` is taken verbatim.
fn collect_patterns(args: impl Iterator<Item = SyntaxResult<RArgument>>, out: &mut Vec<String>) {
    for item in args {
        let Ok(arg) = item else { continue };
        let Some(value) = arg.value() else { continue };
        let pattern = match &value {
            AnyRExpression::RIdentifier(ident) => ident.name_text(),
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(string)) => {
                let (Ok(token), Ok(text)) = (string.value(), string.inner_string_text()) else {
                    continue;
                };
                if token.kind() == R_RAW_STRING_LITERAL {
                    out.push(text.to_string());
                    continue;
                }
                let Some(pattern) = decode_pattern(text.text()) else {
                    log::warn!("Skipping `exportPattern()` with unsupported escape: {text}");
                    continue;
                };
                pattern
            },
            _ => continue,
        };
        out.push(pattern);
    }
}

/// Decode `\\` and escaped quotes. Return `None` for any other escape.
fn decode_pattern(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            escaped @ ('\\' | '"' | '\'') => out.push(escaped),
            _ => return None,
        }
    }
    Some(out)
}

fn directive_name(value: &AnyRExpression) -> Option<String> {
    match value {
        AnyRExpression::RIdentifier(ident) => Some(ident.name_text()),
        AnyRExpression::AnyRValue(AnyRValue::RStringValue(string)) => {
            Some(string.inner_string_text().ok()?.to_string())
        },
        _ => None,
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
        assert_eq!(parsed.exports.to_vec(), vec!["bar", "foo"]);
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
        assert_eq!(parsed.imports, vec![
            Import {
                name: "head".to_string(),
                package: "utils".to_string()
            },
            Import {
                name: "median".to_string(),
                package: "stats".to_string()
            },
        ]);
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
        assert_eq!(parsed.exports.to_vec(), vec!["bar", "foo"]);
        assert_eq!(parsed.imports, vec![
            Import {
                name: "head".to_string(),
                package: "utils".to_string()
            },
            Import {
                name: "median".to_string(),
                package: "stats".to_string()
            },
        ]);
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
        assert_eq!(parsed.exports.to_vec(), vec!["foo"]);
        assert_eq!(parsed.imports, vec![Import {
            name: "median".to_string(),
            package: "stats".to_string()
        }]);
    }

    #[test]
    fn parses_multiple_args() {
        let ns = r#"
                import(foo, bar)
                export(baz, qux)
                importFrom(pkg, a, b, c)
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.imports, vec![
            Import {
                name: "a".to_string(),
                package: "pkg".to_string()
            },
            Import {
                name: "b".to_string(),
                package: "pkg".to_string()
            },
            Import {
                name: "c".to_string(),
                package: "pkg".to_string()
            },
        ]);
        assert_eq!(parsed.package_imports, vec!["bar", "foo"]);
        assert_eq!(parsed.exports.to_vec(), vec!["baz", "qux"]);
    }

    #[test]
    fn parses_s4_exports() {
        let ns = r#"
                exportClasses(foo)
                exportClasses(bar, baz)
                exportMethods(qux)
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.exports.to_vec(), vec!["qux"]);
    }

    #[test]
    fn parses_quoted_exports() {
        let ns = r#"
                export("foo", "bar")
                exportClasses("baz")
                exportMethods("qux")
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.exports.to_vec(), vec!["bar", "foo", "qux"]);
    }

    #[test]
    fn parses_export_patterns() {
        let ns = r#"
                exportPattern("^[^\\.]")
                export(foo)
                exportPattern("^as_", 'quote\'d', "\"x\"")
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.export_patterns, vec![
            r"^[^\.]", "^as_", "quote'd", r#""x""#
        ]);
        assert_eq!(parsed.exports.to_vec(), vec!["foo"]);
    }

    #[test]
    fn parses_raw_string_export_patterns_verbatim() {
        let ns = r#"
                exportPattern(r"(^[^\.])", r"-[\\x]-")
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.export_patterns, vec![r"^[^\.]", r"\\x"]);

        let quoted = Namespace::parse(r#"exportPattern("^[^\\.]")"#).unwrap();
        assert_eq!(quoted.export_patterns[0], parsed.export_patterns[0]);
    }

    #[test]
    fn skips_export_pattern_with_unsupported_escape() {
        let ns = r#"
                exportPattern("^\\.x", "\t", "^y")
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.export_patterns, vec![r"^\.x", "^y"]);
    }

    #[test]
    fn parses_exports_mixing_quoted_and_unquoted() {
        let ns = r#"
                export("toTitleCase", langElts, 'file_ext')
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.exports.to_vec(), vec![
            "file_ext",
            "langElts",
            "toTitleCase"
        ]);
    }

    #[test]
    fn parses_non_syntactic_exports() {
        let ns = r#"
                export("%>%")
                export("[.myclass", "+.money")
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.exports.to_vec(), vec!["%>%", "+.money", "[.myclass"]);
    }

    #[test]
    fn parses_quoted_importfrom() {
        let ns = r#"
                importFrom("stats", "median", head)
                importFrom(utils, "tail")
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.imports, vec![
            Import {
                name: "head".to_string(),
                package: "stats".to_string()
            },
            Import {
                name: "median".to_string(),
                package: "stats".to_string()
            },
            Import {
                name: "tail".to_string(),
                package: "utils".to_string()
            },
        ]);
    }

    #[test]
    fn parses_quoted_bulk_imports() {
        let ns = r#"
                import("rlang")
                import(cli, "utils")
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.package_imports, vec!["cli", "rlang", "utils"]);
    }

    #[test]
    fn parses_directive_ignoring_non_name_arguments() {
        let ns = r#"
                import(rlang, except = c(abort))
                export(foo, 42)
            "#;
        let parsed = Namespace::parse(ns).unwrap();
        assert_eq!(parsed.package_imports, vec!["rlang"]);
        assert_eq!(parsed.exports.to_vec(), vec!["foo"]);
    }
}
