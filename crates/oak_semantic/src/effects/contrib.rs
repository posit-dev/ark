use std::sync::LazyLock;

use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::AnyRExpression;
use aether_syntax::RSyntaxKind;
use biome_rowan::AstNodeList;
use oak_core::syntax_ext::RIdentifierExt;
use rustc_hash::FxHashMap;

use crate::effects::declare::parse_declaration;
use crate::effects::declare::DeclareDiagnostic;
use crate::effects::Declaration;
use crate::effects::EffectSource;
use crate::effects::Handler;

mod base;
mod magrittr;
mod rlang;
mod s7;

/// One registry entry: a `(package, function)` pair and the effect source it
/// contributes. Owns its payload, an inline [`Declaration`] (data) or a
/// `&'static dyn Handler` (code), matching the [`EffectSource`] split. The
/// function name is owned because declarative entries carry the name parsed from
/// their `.ty.R` stub.
pub(crate) struct Entry {
    pub(super) package: &'static str,
    pub(super) function: String,
    payload: EntryPayload,
}

/// A registry entry is declarative xor custom, never per-axis mixable: in
/// practice a function is all-declarative (`local`, `library`) or all-custom
/// (bquote, binding operators, assign), never one axis of each.
enum EntryPayload {
    Declared(Declaration),
    Custom(&'static dyn Handler),
}

impl Entry {
    /// Project an entry into the `Copy` [`EffectSource`]. The `&'static`
    /// borrows come from the `LazyLock`-owned registry.
    pub(super) fn source(&'static self) -> EffectSource {
        match &self.payload {
            EntryPayload::Declared(declaration) => EffectSource::Declared(declaration),
            EntryPayload::Custom(handler) => EffectSource::Custom(*handler),
        }
    }
}

/// Build a custom entry from a `&'static dyn Handler`. Declarative entries come
/// from the `.ty.R` stubs, so this is the only builder the Rust contrib modules
/// use.
pub(super) fn custom(
    package: &'static str,
    function: &'static str,
    handler: &'static dyn Handler,
) -> Entry {
    Entry {
        package,
        function: function.to_string(),
        payload: EntryPayload::Custom(handler),
    }
}

/// A package's bundled `.ty.R` stub: its package name (attribution comes from the
/// file, not the parsed text) and R source.
struct Stub {
    package: &'static str,
    source: &'static str,
}

/// The bundled `.ty.R` stubs, `include_str!`-ed so the same `declare()` parser
/// that reads local declarations reads the registry too.
const STUBS: &[Stub] = &[
    Stub {
        package: "base",
        source: include_str!("contrib/base.ty.R"),
    },
    Stub {
        package: "shiny",
        source: include_str!("contrib/shiny.ty.R"),
    },
    Stub {
        package: "rlang",
        source: include_str!("contrib/rlang.ty.R"),
    },
    Stub {
        package: "testthat",
        source: include_str!("contrib/testthat.ty.R"),
    },
];

/// One declared function parsed out of an `.ty.R` stub: its name and the
/// declaration, plus any diagnostics the parse collected.
struct StubEntry {
    function: String,
    declaration: Declaration,
    diagnostics: Vec<DeclareDiagnostic>,
}

/// Parse one `.ty.R` stub into its declared entries. Each top-level
/// `name <- function(...) declare(...)` assignment becomes one entry, its name
/// taken from the assignment target and its declaration from
/// [`parse_declaration`]. A function with no `declare()` directive is skipped.
fn parse_stub(source: &str) -> Vec<StubEntry> {
    let parsed = parse(source, RParserOptions::default());
    parsed
        .tree()
        .expressions()
        .iter()
        .filter_map(|expr| stub_entry(&expr))
        .collect()
}

/// A top-level `name <- function(...) ...` assignment turned into a
/// [`StubEntry`], or `None` when the statement isn't such an assignment or the
/// function carries no `declare()` directive.
fn stub_entry(expr: &AnyRExpression) -> Option<StubEntry> {
    let AnyRExpression::RBinaryExpression(bin) = expr else {
        return None;
    };
    if bin.operator().ok()?.kind() != RSyntaxKind::ASSIGN {
        return None;
    }
    let AnyRExpression::RIdentifier(name) = bin.left().ok()? else {
        return None;
    };
    let AnyRExpression::RFunctionDefinition(function) = bin.right().ok()? else {
        return None;
    };
    let parsed = parse_declaration(&function)?;
    Some(StubEntry {
        function: name.name_text(),
        declaration: parsed.declaration,
        diagnostics: parsed.diagnostics,
    })
}

/// The effect registry: every package's contributions plus a name-keyed index.
///
/// `by_function` maps a function name to the entries carrying it. Its keys double
/// as the name gate [`annotates`] consults, and its values disambiguate by
/// package for [`lookup`], so both queries are a single hash lookup rather than a
/// scan over every entry.
pub(super) struct Registry {
    entries: Vec<Entry>,
    by_function: FxHashMap<String, Vec<usize>>,
}

impl Registry {
    fn new(entries: Vec<Entry>) -> Self {
        let mut by_function: FxHashMap<String, Vec<usize>> = FxHashMap::default();
        for (idx, entry) in entries.iter().enumerate() {
            by_function
                .entry(entry.function.clone())
                .or_default()
                .push(idx);
        }
        Registry {
            entries,
            by_function,
        }
    }
}

/// The effect registry, assembled once. Custom handlers come from the Rust
/// contrib modules; declarative entries come from the bundled `.ty.R` stubs,
/// parsed by the same `declare()` parser local declarations use.
///
/// A `LazyLock<Registry>` rather than a `static` table because the parsed
/// [`Declaration`]s hold `Vec`s and can't sit in a `static`.
pub(super) static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
    let mut entries: Vec<Entry> = [
        base::entries(),
        magrittr::entries(),
        rlang::entries(),
        s7::entries(),
    ]
    .into_iter()
    .flatten()
    .collect();

    for stub in STUBS {
        for stub_entry in parse_stub(stub.source) {
            // A well-formed stub yields no diagnostics. `all_stubs_parse_without_diagnostics`
            // is the real gate; the log is for anyone who edits a stub and runs
            // outside the test.
            if !stub_entry.diagnostics.is_empty() {
                log::warn!(
                    "Effect stub {}::{} has declare() diagnostics: {:?}",
                    stub.package,
                    stub_entry.function,
                    stub_entry.diagnostics
                );
            }
            entries.push(Entry {
                package: stub.package,
                function: stub_entry.function,
                payload: EntryPayload::Declared(stub_entry.declaration),
            });
        }
    }

    Registry::new(entries)
});

/// Look up the effect source of a `(package, function)` pair.
pub(super) fn lookup(package: &str, function: &str) -> Option<EffectSource> {
    let registry: &'static Registry = &REGISTRY;
    let indices = registry.by_function.get(function)?;
    indices
        .iter()
        .map(|&idx| &registry.entries[idx])
        .find(|entry| entry.package == package)
        .map(Entry::source)
}

/// Whether any registry entry annotates `name`.
pub(super) fn annotates(name: &str) -> bool {
    REGISTRY.by_function.contains_key(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a stub and return each entry's name paired with its diagnostics, so
    /// a test can assert both the expected functions and that none of them
    /// tripped a `declare()` diagnostic.
    fn stub_report(source: &str) -> Vec<(String, Vec<DeclareDiagnostic>)> {
        parse_stub(source)
            .into_iter()
            .map(|entry| (entry.function, entry.diagnostics))
            .collect()
    }

    fn stub_functions(source: &str) -> Vec<String> {
        stub_report(source)
            .into_iter()
            .map(|(function, _)| function)
            .collect()
    }

    /// Every bundled stub must parse cleanly: zero diagnostics recovers the
    /// compile-time validation the handcrafted Rust entries used to give.
    #[test]
    fn all_stubs_parse_without_diagnostics() {
        for stub in STUBS {
            for (function, diagnostics) in stub_report(stub.source) {
                assert_eq!(diagnostics, Vec::new(), "in {}::{function}", stub.package);
            }
        }
    }

    #[test]
    fn base_stub_declares_expected_functions() {
        assert_eq!(stub_functions(include_str!("contrib/base.ty.R")), vec![
            "evalq",
            "local",
            "with",
            "with.default",
            "within",
            "within.data.frame",
            "quote",
            "library",
            "require",
            "source",
        ]);
    }

    #[test]
    fn shiny_stub_declares_expected_functions() {
        assert_eq!(stub_functions(include_str!("contrib/shiny.ty.R")), vec![
            "observe",
            "reactive",
            "renderPlot",
            "renderPrint",
            "renderTable",
            "renderText",
            "renderUI",
        ]);
    }

    #[test]
    fn rlang_stub_declares_expected_functions() {
        assert_eq!(stub_functions(include_str!("contrib/rlang.ty.R")), vec![
            "on_load"
        ]);
    }

    #[test]
    fn testthat_stub_declares_expected_functions() {
        assert_eq!(stub_functions(include_str!("contrib/testthat.ty.R")), vec![
            "test_that"
        ]);
    }
}
