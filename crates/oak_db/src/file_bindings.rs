use std::collections::BTreeMap;

use aether_syntax::AnyRExpression;
use aether_syntax::RSyntaxNode;
use oak_core::syntax_ext::RIdentifierExt;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::SemanticIndex;

use crate::definition::value_node;
use crate::Db;
use crate::ExportEntry;
use crate::File;
use crate::Package;

/// What a top-level name is bound to, as far as the syntax shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// Every indexed definition assigns a function expression or a
    /// `.Primitive()` call, including values passed to `assign()`.
    Function,
    /// At least one binding has an unclassified value or is forwarded by
    /// `source()`. This includes non-function values, aliases, and names with
    /// both function and non-function definitions.
    Unknown,
}

impl BindingKind {
    /// The kind of a name bound both as `self` and as `other`: a function only
    /// if both are. At runtime the last binding usually wins, but conditional
    /// bindings and `.onLoad()` overrides make "last" unreliable, so any doubt
    /// gives unknown.
    pub fn merge(self, other: BindingKind) -> BindingKind {
        match (self, other) {
            (BindingKind::Function, BindingKind::Function) => BindingKind::Function,
            _ => BindingKind::Unknown,
        }
    }
}

/// Top-level names with their [`BindingKind`], sorted by name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Bindings {
    entries: BTreeMap<String, BindingKind>,
}

impl Bindings {
    pub(crate) fn from_entries(entries: BTreeMap<String, BindingKind>) -> Self {
        Self { entries }
    }

    pub fn get(&self, name: &str) -> Option<BindingKind> {
        self.entries.get(name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, BindingKind)> {
        self.entries
            .iter()
            .map(|(name, kind)| (name.as_str(), *kind))
    }

    fn insert(&mut self, name: &str, kind: BindingKind) {
        match self.entries.get_mut(name) {
            Some(existing) => *existing = existing.merge(kind),
            None => {
                self.entries.insert(name.to_string(), kind);
            },
        }
    }
}

#[salsa::tracked]
impl File {
    /// Classify the names from [`File::exports()`] using local definitions.
    /// Names forwarded by `source()` have unknown kinds.
    ///
    /// Keeping kinds in a separate query prevents a kind-only edit, such as
    /// `f <- 1` becoming `f <- function() 1`, from invalidating name resolution.
    /// Caching avoids recomputing kinds from [`File::parse()`], whose syntax
    /// trees may be evicted. Unchanged names and kinds retain the query's
    /// result revision.
    #[salsa::tracked(returns(ref))]
    pub fn bindings(self, db: &dyn Db) -> Bindings {
        let index = self.semantic_index(db);
        let root = self.parse(db).syntax();

        let mut bindings = Bindings::default();
        for (name, entries) in self.exports(db).iter() {
            let kind = if entries.iter().all(|entry| *entry == ExportEntry::Local) {
                local_kind(index, &root, name)
            } else {
                BindingKind::Unknown
            };
            bindings.insert(name, kind);
        }
        bindings
    }
}

#[salsa::tracked]
impl Package {
    /// Merge [`File::bindings()`] across [`Package::files()`]. A name bound in
    /// several files is a function only if every file classifies it as one.
    ///
    /// Re-exports through `importFrom()` and `export()` have no local binding.
    /// [`Package::resolve()`] follows them to their source package. This query
    /// does not follow re-exports, so it cannot enter a re-export cycle.
    #[salsa::tracked(returns(ref))]
    pub fn bindings(self, db: &dyn Db) -> Bindings {
        let mut bindings = Bindings::default();
        for file in self.files(db) {
            for (name, kind) in file.bindings(db).iter() {
                bindings.insert(name, kind);
            }
        }
        bindings
    }
}

// `source()` imports are classified separately through `ExportEntry::Import`.
// Only local definitions contribute to this kind.
fn local_kind(index: &SemanticIndex, root: &RSyntaxNode, name: &str) -> BindingKind {
    index
        .export(name)
        .filter(|(_, def)| !matches!(def.kind(), DefinitionKind::Import { .. }))
        .map(|(_, def)| definition_kind(def.kind(), root))
        .reduce(BindingKind::merge)
        .unwrap_or(BindingKind::Unknown)
}

fn definition_kind(kind: &DefinitionKind, root: &RSyntaxNode) -> BindingKind {
    match value_node(kind, root) {
        Some(value) if is_function_value(&value) => BindingKind::Function,
        _ => BindingKind::Unknown,
    }
}

fn is_function_value(value: &AnyRExpression) -> bool {
    match value {
        AnyRExpression::RFunctionDefinition(_) => true,
        AnyRExpression::RCall(call) => matches!(
            call.function(),
            Ok(AnyRExpression::RIdentifier(ident)) if ident.name_text() == ".Primitive"
        ),
        _ => false,
    }
}
