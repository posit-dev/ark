use url::Url;

use crate::effects::Effects;
use crate::effects_registry;

/// The result of resolving a `source()` call. Returned by
/// [`ImportsResolver::resolve_source`].
#[derive(Clone)]
pub struct SourceResolution {
    /// The resolved URL of the sourced file.
    pub url: Url,

    /// Names of top-level definitions in the sourced file.
    pub names: Vec<String>,

    /// Package names from `library()` directives in the sourced file
    /// (and transitively from files it sources).
    pub packages: Vec<String>,
}

/// Resolves the imports of the file currently being indexed.
///
/// The builder owns local definitions (it's building them as it walks).
/// Anything visible to the file from outside (`source()` injections,
/// `library()` attaches, NAMESPACE imports, the default search path) is
/// an import, and the builder consults this trait whenever it needs to
/// know about one. Concrete impls live in their host crate:
///
/// - [`NoopImportsResolver`]: no imports. The builder records local definitions
///   and `source()` call sites but injects no cross-file bindings. Suitable
///   for isolated indexing (CLI tools, unit tests).
/// - `oak_db::SalsaImportsResolver`: salsa-backed lookup against the source graph.
///
/// The trait has three queries:
///
/// - [`resolve_source`](ImportsResolver::resolve_source) is the bulk
///   query, "enumerate every name this `source("path")` brings in," used
///   to inject `DefinitionKind::Import` entries at each source() offset.
/// - [`resolve_effects`](ImportsResolver::resolve_effects) resolves a bare
///   callee against imports, e.g. the search path, and returns known effects.
/// - [`resolve_qualified_effects`](ImportsResolver::resolve_qualified_effects)
///   resolves the effects of a `pkg::fn` (or `:::) callee against a named package.
pub trait ImportsResolver {
    /// Resolve a `source("path")` call to the target file's exported names
    /// and transitive `library()` attachments. The path is the literal
    /// string in the `source()` call and the resolver is responsible for
    /// anchoring it (workspace root, calling file's directory, ...).
    /// Returns `None` when the target can't be located.
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution>;

    /// Resolve a bare callee `name` to its effects. The builder state is passed
    /// in because the resolver can't query our own semantic index without
    /// creating a cycle:
    ///
    /// - `attached`: packages attached at this point, in flow order.
    /// - `lazy`: whether the callee sits in a lazy context like a function.
    fn resolve_effects(&mut self, name: &str, attached: &[String], lazy: bool) -> Option<Effects> {
        let _ = (name, attached, lazy);
        None
    }

    /// Resolve a namespace-qualified callee `pkg::fn` (or equivalently with
    /// `:::`) to its effects.
    fn resolve_qualified_effects(&mut self, package: &str, name: &str) -> Option<Effects> {
        effects_registry::lookup(package, name)
            .copied()
            .map(Effects::nse)
    }
}

/// Resolver that returns nothing. The builder skips all cross-file
/// injection, which is the desired behavior when callers don't have or
/// don't want cross-file context.
pub struct NoopImportsResolver;

impl ImportsResolver for NoopImportsResolver {
    fn resolve_source(&mut self, _path: &str) -> Option<SourceResolution> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_noop_imports_resolver_returns_none() {
        // Contract: `NoopImportsResolver` returns `None` for every input.
        // The builder's behavior under this contract is exercised by
        // every builder test that uses the default `index()` helper
        // (see `tests/builder.rs`), but the contract itself is named
        // here so a change to the trait method's signature can't
        // silently break it.
        let mut resolver = NoopImportsResolver;
        assert!(resolver.resolve_source("").is_none());
        assert!(resolver.resolve_source("relative.R").is_none());
        assert!(resolver.resolve_source("/abs/path.R").is_none());
        assert!(resolver.resolve_source("../../up.R").is_none());
    }
}
