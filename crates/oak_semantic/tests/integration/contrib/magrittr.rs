use aether_parser::parse;
use aether_parser::RParserOptions;
use biome_rowan::AstNode;
use oak_semantic::build_index;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::UseId;

use crate::common::index_with_attached;
use crate::common::index_with_base;
use crate::common::only_assign_def;
use crate::resolvers::TestImportsResolver;

#[test]
fn test_magrittr_compound_assignment_binds_lhs() {
    // `x %<>% f()` binds `x`. The left operand is a definition target, not a
    // read, the same as `<-`, so only `f` (the right operand) is a use.
    //
    // `%<>%` is compound (`x <- f(x)`), so `x` is conceptually read too, but we
    // don't record that read yet. See the `TODO(nse)` in the walk's binary arm.
    let index = index_with_attached("x %<>% f()", &["magrittr"]);
    let file = ScopeId::from(0);

    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));

    // Only the right operand `f` is a use; the target `x` is not recorded.
    assert_eq!(index.uses(file).len(), 1);
    let symbols = index.symbols(file);
    assert_eq!(
        symbols
            .symbol(index.uses(file)[UseId::from(0)].symbol())
            .name(),
        "f"
    );
}

#[test]
fn test_magrittr_compound_assignment_name_and_value_handles() {
    // The `name` handle is the left operand (goto, rename), the `value` handle
    // the right operand.
    let source = "x %<>% f()";
    let parsed = parse(source, RParserOptions::default());
    assert!(!parsed.has_error());
    let root = parsed.tree().syntax().clone();
    let index = build_index(
        &parsed.tree(),
        TestImportsResolver::with_attached(&["magrittr"]),
    );

    let Some(DefinitionKind::Assign {
        name,
        value: Some(value),
        ..
    }) = only_assign_def(&index)
    else {
        panic!("expected an assign def with a value handle");
    };
    assert_eq!(name.to_node(&root).syntax().text_trimmed().to_string(), "x");
    assert_eq!(
        value.to_node(&root).syntax().text_trimmed().to_string(),
        "f()"
    );
}

#[test]
fn test_plain_operators_do_not_bind() {
    // A pipe and a plain infix operator are not assign effects, even with
    // magrittr attached: the match is on the exact operator text.
    assert!(only_assign_def(&index_with_attached("x %>% f()", &["magrittr"])).is_none());
    assert!(only_assign_def(&index_with_attached("x %in% y", &["magrittr"])).is_none());
}

#[test]
fn test_compound_assignment_shadowed_by_local_binding_not_recognized() {
    // A user-defined `%<>%` shadows magrittr's, so the operator binds nothing.
    let index = index_with_attached("`%<>%` <- function(lhs, rhs) {}\nx %<>% f()", &["magrittr"]);
    assert!(only_assign_def(&index).is_none());
}

#[test]
fn test_compound_assignment_complex_lhs_not_recorded() {
    // A complex target binds no name, matching `<-` on `x$y <- v`.
    let index = index_with_attached("x$y %<>% f()", &["magrittr"]);
    assert!(only_assign_def(&index).is_none());
}

#[test]
fn test_compound_assignment_not_recognized_without_magrittr() {
    // Package-aware: without magrittr attached, `%<>%` doesn't resolve, so it
    // stays a plain operator and binds nothing.
    let index = index_with_base("x %<>% f()");
    assert!(only_assign_def(&index).is_none());
}

#[test]
fn test_compound_assignment_use_resolves_to_binding() {
    // A later use resolves to the operator's binding, the same as `assign()`.
    let index = index_with_attached("y %<>% f()\ny", &["magrittr"]);
    let file = ScopeId::from(0);

    // Uses in order: `f` (right operand), then the trailing `y`. The left
    // operand `y` is the binding target, not a use.
    let map = index.use_def_map(file);
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions().len(), 1);
    let def = &index.definitions(file)[bindings.definitions()[0]];
    assert!(matches!(def.kind(), DefinitionKind::Assign { .. }));
}

#[test]
fn test_compound_assignment_definition_range_is_name() {
    // The def's range is the left operand, so `definition_at` locates it when the
    // cursor is on the name at the definition site (not an empty span).
    let index = index_with_attached("value %<>% f()", &["magrittr"]);
    let (scope, _id, def) = index
        .definition_at(biome_rowan::TextSize::from(0))
        .expect("assign def at the name offset");
    assert_eq!(scope, ScopeId::from(0));
    assert!(matches!(def.kind(), DefinitionKind::Assign { .. }));
}

#[test]
fn test_compound_assignment_binding_masks_later_callee() {
    // `local %<>% f()` binds `local`, so the later `local({ ... })` is shadowed
    // and not treated as NSE (no nested scope pushed). The correctness win,
    // parallel to `assign("local", identity)` masking base `local`.
    let index = index_with_attached("local %<>% f()\nlocal({ x <- 1 })", &["magrittr"]);
    assert_eq!(index.scope_ids().count(), 1);
}
