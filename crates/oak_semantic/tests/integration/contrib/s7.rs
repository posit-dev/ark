use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::UseId;

use crate::common::index_with_attached;
use crate::common::only_assign_def;

#[test]
fn test_s7_walrus_assignment_binds_lhs() {
    // S7's `:=` binds its left operand, like `%<>%`. It's the `WALRUS` token
    // kind rather than `SPECIAL`, so it exercises the other arm of the operator
    // gate.
    let index = index_with_attached("x := f()", &["S7"]);
    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));
}

#[test]
fn test_binding_operator_left_operand_is_not_a_use() {
    // A pure-binding operator (`:=` is `x <- expr`, not compound) reads only its
    // right operand. The left operand `x` is the binding target, not a use, so
    // exactly one use is recorded and it's the value operand.
    let index = index_with_attached("x := f()", &["S7"]);
    let file = ScopeId::from(0);

    assert_eq!(index.uses(file).len(), 1);
    assert_eq!(
        index
            .symbols(file)
            .symbol(index.uses(file)[UseId::from(0)].symbol())
            .name(),
        "f"
    );
}
