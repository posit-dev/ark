use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::RCall;
use biome_rowan::AstSeparatedList;
use oak_semantic::build_index;
use oak_semantic::effects::AssignBinding;
use oak_semantic::effects::AssignHandler;
use oak_semantic::effects::CallContext;
use oak_semantic::effects::EffectHandler;
use oak_semantic::effects::EffectSite;
use oak_semantic::effects::RangedAstPtr;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::NoopImportsResolver;

use crate::resolvers::TestImportsResolver;

pub(crate) fn index(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    build_index(&parsed.tree(), NoopImportsResolver)
}

/// Build with base attached. Attach recognition (`library()`/`require()`) runs
/// on the resolve path now, so it needs a resolver that resolves base, unlike
/// the resolver-independent `source()` recognition the `index()` helper covers.
pub(crate) fn index_with_base(source: &str) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());

    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }

    build_index(&parsed.tree(), TestImportsResolver::with_base())
}

/// Build with `packages` attached (plus base), for package-contributed effects
/// like magrittr's `%<>%` operator.
pub(crate) fn index_with_attached(source: &str, packages: &[&str]) -> SemanticIndex {
    let parsed = parse(source, RParserOptions::default());
    if parsed.has_error() {
        panic!("source has syntax errors: {source}");
    }
    build_index(&parsed.tree(), TestImportsResolver::with_attached(packages))
}

pub(crate) fn semantic_call_kinds(index: &SemanticIndex) -> Vec<&SemanticCallKind> {
    index.semantic_calls().iter().map(|c| c.kind()).collect()
}

/// The single `DefinitionKind::Assign` def in a file, or `None`.
pub(crate) fn only_assign_def(index: &SemanticIndex) -> Option<&DefinitionKind> {
    let file = ScopeId::from(0);
    let mut defs = index
        .definitions(file)
        .iter()
        .map(|(_, def)| def.kind())
        .filter(|kind| matches!(kind, DefinitionKind::Assign { .. }));
    let first = defs.next();
    assert!(defs.next().is_none());
    first
}

/// A source handler that resolves one call to a fixed collation of files,
/// standing in for a collation-style callee. Attached to the `source` name
/// (which passes the `annotates()` front gate) by a resolver under test.
#[derive(Debug)]
pub(crate) struct CollationHandler;

pub(crate) static COLLATION_HANDLER: CollationHandler = CollationHandler;

impl EffectHandler for CollationHandler {
    type Output = Vec<String>;

    fn resolve(&self, _call: &RCall, _ctx: &CallContext<'_>) -> Option<Vec<String>> {
        Some(vec!["a.R".into(), "b.R".into()])
    }
}

/// An assign handler that binds a fixed set of names, standing in for a
/// multi-binding callee. Attached to `assign` by a resolver under test.
#[derive(Debug)]
pub(crate) struct MultiAssignHandler;

pub(crate) static MULTI_ASSIGN_HANDLER: MultiAssignHandler = MultiAssignHandler;

impl AssignHandler for MultiAssignHandler {
    fn resolve(&self, site: EffectSite, _ctx: &CallContext<'_>) -> Option<Vec<AssignBinding>> {
        let EffectSite::Call(call) = site else {
            return None;
        };
        // Point every binding's handles at the first argument. This test only
        // checks that multiple defs are created and resolve, not their ranges.
        let expr = call
            .arguments()
            .ok()?
            .items()
            .iter()
            .next()?
            .ok()?
            .value()?;
        let ptr = RangedAstPtr::new(&expr);
        Some(vec![
            AssignBinding {
                name: "a".into(),
                name_expr: ptr.clone(),
                value_expr: None,
            },
            AssignBinding {
                name: "b".into(),
                name_expr: ptr,
                value_expr: None,
            },
        ])
    }
}
