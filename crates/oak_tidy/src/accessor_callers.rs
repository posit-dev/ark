use std::fs;
use std::path::Path;

use proc_macro2::Delimiter;
use proc_macro2::Spacing;
use proc_macro2::TokenStream;
use proc_macro2::TokenTree;
use syn::visit;
use syn::visit::Visit;
use syn::Attribute;
use syn::ExprMethodCall;
use syn::ExprPath;
use syn::ImplItemFn;
use syn::ItemFn;
use syn::ItemImpl;
use syn::ItemMod;
use syn::ItemTrait;
use syn::Macro;
use syn::TraitItemFn;
use syn::Type;
use walkdir::WalkDir;

use crate::salsa_inventory::has_cfg_test;
use crate::salsa_inventory::is_test_path;
use crate::salsa_inventory::normalize_tokens;

pub const ACCESSOR_CALLERS_CHECKLIST: &str = "\
Updating this snapshot means code outside `oak_db` gained or lost a call to a broad accessor such as `File::syntax_tree()`. For each new caller, check whether a narrow `oak_db` query answers its question instead. If none does, ask the human before accepting the snapshot.";

/// Broad accessors are matched by name because `syn` does not resolve types.
/// Each name must be unique in the workspace to avoid unrelated matches.
const BROAD_ACCESSORS: [&str; 1] = ["syntax_tree"];

/// Calls within the owning crate are implementation details, not external
/// callers subject to review.
const OWNING_CRATE: &str = "oak_db";

/// Returns a stable list of calls to broad `oak_db` accessors from the
/// non-test sources of the crates below `crates_dir`, one
/// `path: enclosing fn -> accessor` line per caller.
///
/// Excludes `oak_db` and test code, using the test exclusions of
/// `salsa_inventory()` and skipping `#[test]` items. New entries prompt review
/// of whether a narrow query would serve the caller. Tests inspecting trees
/// do not need that review.
pub fn accessor_callers(crates_dir: &Path) -> String {
    let mut crate_dirs = match fs::read_dir(crates_dir) {
        Ok(entries) => entries
            .map(|entry| match entry {
                Ok(entry) => entry.path(),
                Err(err) => panic!("failed to read {}: {err}", crates_dir.display()),
            })
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>(),
        Err(err) => panic!("failed to read {}: {err}", crates_dir.display()),
    };
    crate_dirs.sort();

    let mut files = Vec::new();

    for crate_dir in crate_dirs {
        let source_dir = crate_dir.join("src");
        if !source_dir.is_dir() {
            continue;
        }

        for entry in WalkDir::new(&source_dir).sort_by_file_name() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => panic!("failed to walk {}: {err}", source_dir.display()),
            };
            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }

            let rel_path = match path.strip_prefix(crates_dir) {
                Ok(rel_path) => rel_path,
                Err(_) => panic!("{} is not under {}", path.display(), crates_dir.display()),
            };
            let rel_path = match rel_path.to_str() {
                Some(rel_path) => rel_path.replace('\\', "/"),
                None => panic!("non-utf8 path: {}", path.display()),
            };

            let source = match fs::read_to_string(path) {
                Ok(source) => source,
                Err(err) => panic!("failed to read {}: {err}", path.display()),
            };

            files.push((rel_path, source));
        }
    }

    render_from_sources(&files)
}

/// Renders the caller list from in-memory `(path, source_text)` pairs, where
/// each path is relative to the crates directory and so starts with the
/// crate's directory name. Applies the same exclusions as
/// `accessor_callers()`.
fn render_from_sources(files: &[(String, String)]) -> String {
    let mut entries = Vec::new();

    for (rel_path, source) in files {
        let path = Path::new(rel_path);
        if path
            .components()
            .next()
            .map(|component| component.as_os_str()) ==
            Some(OWNING_CRATE.as_ref())
        {
            continue;
        }
        if is_test_path(path) {
            continue;
        }

        let file = match syn::parse_file(source) {
            Ok(file) => file,
            Err(err) => panic!("failed to parse {rel_path}: {err}"),
        };

        let mut visitor = CallerVisitor::default();
        visitor.visit_file(&file);

        for (caller, accessor) in visitor.calls {
            entries.push(format!("{rel_path}: {caller} -> {accessor}"));
        }
    }

    entries.sort();
    entries.dedup();

    let mut lines = vec!["== broad accessor callers ==".to_string()];
    lines.extend(entries);
    lines.join("\n")
}

/// Collects `(enclosing item path, accessor)` pairs. Paths include inline
/// modules, impl types, traits and function names. Closures retain the
/// enclosing path, while named nested functions add their own segment.
#[derive(Default)]
struct CallerVisitor {
    scope: Vec<String>,
    calls: Vec<(String, &'static str)>,
}

impl CallerVisitor {
    fn record(&mut self, name: &str) {
        let Some(accessor) = BROAD_ACCESSORS.iter().find(|accessor| **accessor == name) else {
            return;
        };
        let caller = if self.scope.is_empty() {
            "<module>".to_string()
        } else {
            self.scope.join("::")
        };
        self.calls.push((caller, accessor));
    }

    fn in_scope(&mut self, name: String, walk: impl FnOnce(&mut Self)) {
        self.scope.push(name);
        walk(self);
        self.scope.pop();
    }
}

impl<'ast> Visit<'ast> for CallerVisitor {
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.in_scope(item.ident.to_string(), |this| {
            visit::visit_item_mod(this, item)
        });
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.in_scope(type_name(&item.self_ty), |this| {
            visit::visit_item_impl(this, item)
        });
    }

    fn visit_item_trait(&mut self, item: &'ast ItemTrait) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.in_scope(item.ident.to_string(), |this| {
            visit::visit_item_trait(this, item)
        });
    }

    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.in_scope(item.sig.ident.to_string(), |this| {
            visit::visit_item_fn(this, item)
        });
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.in_scope(item.sig.ident.to_string(), |this| {
            visit::visit_impl_item_fn(this, item)
        });
    }

    fn visit_trait_item_fn(&mut self, item: &'ast TraitItemFn) {
        if is_test_item(&item.attrs) {
            return;
        }
        self.in_scope(item.sig.ident.to_string(), |this| {
            visit::visit_trait_item_fn(this, item)
        });
    }

    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        self.record(&call.method.to_string());
        visit::visit_expr_method_call(self, call);
    }

    /// Covers path calls (`File::syntax_tree(file, db)`) and function
    /// references (`.map(File::syntax_tree)`), since a call's callee is an
    /// `ExprPath` too. A single-segment path is a local or a free function,
    /// not the method, so a variable named `syntax_tree` isn't listed.
    fn visit_expr_path(&mut self, expr: &'ast ExprPath) {
        if expr.qself.is_some() || expr.path.segments.len() > 1 {
            if let Some(segment) = expr.path.segments.last() {
                self.record(&segment.ident.to_string());
            }
        }
        visit::visit_expr_path(self, expr);
    }

    /// `syn` doesn't parse macro arguments into expressions, so calls inside
    /// `format!()`, `assert_eq!()` and the like are found by scanning tokens.
    fn visit_macro(&mut self, mac: &'ast Macro) {
        for name in accessor_names_in_tokens(mac.tokens.clone()) {
            self.record(&name);
        }
        visit::visit_macro(self, mac);
    }
}

/// Exclude standalone `#[test]` functions as well as `#[cfg(test)]` items.
fn is_test_item(attrs: &[Attribute]) -> bool {
    has_cfg_test(attrs) || attrs.iter().any(|attr| attr.path().is_ident("test"))
}

/// Names the impl block by its type's last path segment, without generics,
/// so lifetime or parameter renames don't churn the list.
fn type_name(ty: &Type) -> String {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident.to_string();
        }
    }
    normalize_tokens(ty)
}

/// Match method calls and qualified paths in macro tokens. Requiring `(` or
/// a turbofish after a method name excludes field accesses.
fn accessor_names_in_tokens(tokens: TokenStream) -> Vec<String> {
    let tokens = tokens.into_iter().collect::<Vec<_>>();
    let mut names = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Group(group) => names.extend(accessor_names_in_tokens(group.stream())),
            TokenTree::Ident(ident) => {
                let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
                let next = tokens.get(index + 1);

                let after_dot =
                    matches!(previous, Some(TokenTree::Punct(punct)) if punct.as_char() == '.');
                let called = match next {
                    Some(TokenTree::Group(group)) => group.delimiter() == Delimiter::Parenthesis,
                    Some(TokenTree::Punct(punct)) => {
                        punct.as_char() == ':' && punct.spacing() == Spacing::Joint
                    },
                    _ => false,
                };
                let after_path_sep = index >= 2 &&
                    matches!(&tokens[index - 1], TokenTree::Punct(punct) if punct.as_char() == ':') &&
                    matches!(&tokens[index - 2], TokenTree::Punct(punct) if punct.as_char() == ':' && punct.spacing() == Spacing::Joint);

                if (after_dot && called) || after_path_sep {
                    names.push(ident.to_string());
                }
            },
            _ => {},
        }
    }

    names
}

#[cfg(test)]
mod tests;
