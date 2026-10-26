//! Projects resolution results onto owned values that can cross database
//! boundaries.
//!
//! Salsa identities are database-local, so the incremental and fresh results
//! compare workspace-relative paths, names, and ranges instead.
//!
//! This projection must not execute queries against the historical database.
//! `Definition::name_range()` calls `File::parse()`, which can warm state under
//! test. Use the `AstPtr` stored by `DefinitionKind` instead, accepting the
//! binding node range, including trivia, rather than the name-token range.

use biome_rowan::AstNode;
use biome_rowan::AstPtr;
use biome_rowan::TextRange;
use oak_semantic::semantic_index::DefinitionKind;

use crate::test_path::path_name;
use crate::Db;
use crate::Definition;

/// Preserve resolution order and duplicates: one name can resolve to several
/// definitions, and `File::resolve()` specifies the last element as R's chosen
/// binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) resolved: Vec<Resolved>,
}

/// Retains ranges to distinguish same-name bindings of the same kind in one
/// file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) kind: KindTag,
    pub(crate) range: TextRange,
    pub(crate) forward: Option<Forward>,
}

/// A [`DefinitionKind`] variant without database-local syntax pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KindTag {
    Assignment,
    SuperAssignment,
    Parameter,
    ForVariable,
    Import,
    Assign,
}

/// Retains an `Import` target to distinguish forwarding bindings with the same
/// name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Forward {
    pub(crate) file: String,
    pub(crate) name: String,
}

pub(crate) fn observe<'db>(db: &'db dyn Db, definitions: &[Definition<'db>]) -> Observation {
    Observation {
        resolved: definitions
            .iter()
            .map(|definition| resolved(db, *definition))
            .collect(),
    }
}

fn resolved<'db>(db: &'db dyn Db, definition: Definition<'db>) -> Resolved {
    let (kind, range, forward) = project(definition.kind(db));

    Resolved {
        path: path_name(definition.file(db).path(db)),
        name: definition.name(db).text(db).to_string(),
        kind,
        range,
        forward,
    }
}

fn project(kind: &DefinitionKind) -> (KindTag, TextRange, Option<Forward>) {
    match kind {
        DefinitionKind::Assignment(ptr) => (KindTag::Assignment, node_range(ptr), None),
        DefinitionKind::SuperAssignment(ptr) => (KindTag::SuperAssignment, node_range(ptr), None),
        DefinitionKind::Parameter(ptr) => (KindTag::Parameter, node_range(ptr), None),
        DefinitionKind::ForVariable(ptr) => (KindTag::ForVariable, node_range(ptr), None),
        DefinitionKind::Import { call, file, name } => {
            // Compare the raw URL text produced by resolution rather than
            // introducing separate path normalization.
            let forward = Forward {
                file: file.as_str().to_string(),
                name: name.clone(),
            };
            (KindTag::Import, node_range(call), Some(forward))
        },
        DefinitionKind::Assign { node, .. } => (KindTag::Assign, node_range(node), None),
    }
}

/// Reads `AstPtr`'s stored source range without resolving it against a syntax
/// tree.
fn node_range<N: AstNode>(ptr: &AstPtr<N>) -> TextRange {
    ptr.syntax_node_ptr().text_range()
}
