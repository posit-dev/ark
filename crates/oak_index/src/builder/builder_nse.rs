use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRSelector;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::TextRange;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use stdext::OptionExt;

use super::SemanticIndexBuilder;
use crate::external::ExternalDefinition;
use crate::nse_registry;
use crate::nse_registry::NseAnnotation;
use crate::nse_registry::ScopedArg;
use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionKind;
use crate::semantic_index::NseScope;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::ScopeLaziness;
use crate::semantic_index::SymbolFlags;

impl<'a> SemanticIndexBuilder<'a> {
    // --- NSE callee resolution ---

    /// Resolve the callee of a call to an NSE annotation.
    ///
    /// - Bare identifier that is unbound in the current scope → look up by
    ///   name across all registered packages.
    /// - `pkg::fn` namespace expression → look up `(pkg, fn)` directly.
    pub(super) fn resolve_nse_callee(&self, call: &RCall) -> Option<&'static NseAnnotation> {
        let Ok(func) = call.function() else {
            return None;
        };

        match &func {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();
                let Some(symbol_id) = self.symbol_tables[self.current_scope].id(&name) else {
                    stdext::debug_panic!(
                        "Callee `{name}` not interned: collect_expression should have run first"
                    );
                    return self.resolve_nse_name(&name);
                };
                if !self.current_use_def.is_unbound(symbol_id) {
                    return None;
                }
                self.resolve_nse_name(&name)
            },
            AnyRExpression::RNamespaceExpression(ns_expr) => {
                let left = ns_expr.left().ok()?;
                let right = ns_expr.right().ok()?;
                let pkg = selector_name(&left)?;
                let func_name = selector_name(&right)?;
                nse_registry::lookup(&pkg, &func_name)
            },
            _ => None,
        }
    }

    fn resolve_nse_name(&self, name: &str) -> Option<&'static NseAnnotation> {
        match self.resolver.resolve(name) {
            Some(ExternalDefinition::Package { ref package, .. }) => {
                nse_registry::lookup(package, name)
            },
            Some(ExternalDefinition::ProjectFile { .. }) => None,
            None => {
                // Base is always attached but may not be discoverable through
                // the resolver (base has no NAMESPACE and its INDEX is
                // incomplete). Fall back to a direct registry check.
                nse_registry::lookup("base", name)
            },
        }
    }

    // --- NSE call handling ---

    /// On the first walk: record ranges of Nested argument bodies so the
    /// re-walk's pre-scan can skip them. Arguments are still processed flat.
    /// Sets `found_nse` when any scope-pushing combo is matched.
    pub(super) fn record_nse_decision(&mut self, call: &RCall, annotation: &'static NseAnnotation) {
        let Ok(args) = call.arguments() else {
            return;
        };
        let mut consumed = vec![false; annotation.scoped_args.len()];

        // First pass: match named arguments
        for item in args.items().iter() {
            let Ok(arg) = item else { continue };
            let Some(scoped_idx) = match_named_arg(&arg, annotation, &consumed) else {
                continue;
            };
            consumed[scoped_idx] = true;
            if let Some(scoped) = annotation.scoped_args.get(scoped_idx) {
                self.record_nse_arg_decision(scoped, arg.value());
            }
        }

        // Second pass: match unnamed arguments by position
        let mut position = 0usize;
        for item in args.items().iter() {
            let Ok(arg) = item else {
                position += 1;
                continue;
            };
            if arg.name_clause().is_some() {
                position += 1;
                continue;
            }
            if let Some(scoped_idx) = match_positional_arg(annotation, position, &consumed) {
                consumed[scoped_idx] = true;
                if let Some(scoped) = annotation.scoped_args.get(scoped_idx) {
                    self.record_nse_arg_decision(scoped, arg.value());
                }
            }
            position += 1;
        }
    }

    /// Record information for a single matched NSE argument. For Nested scoped
    /// args, records the body range so the re-walk pre-scan can skip it. For
    /// any scope-pushing combo, sets `found_nse`.
    fn record_nse_arg_decision(&mut self, scoped: &ScopedArg, value: Option<AnyRExpression>) {
        // `Current + Eager` doesn't push a scope, nothing to record.
        if scoped.nse_scope == NseScope::Current && scoped.laziness == ScopeLaziness::Eager {
            return;
        }

        self.found_nse = true;

        if scoped.nse_scope == NseScope::Nested {
            if let Some(value) = value {
                self.nse_nested_ranges
                    .insert(value.syntax().text_trimmed_range());
            }
        }
    }

    /// On the re-walk: push scopes for annotated arguments and walk their
    /// bodies inside the new scope.
    pub(super) fn collect_nse_call(&mut self, call: &RCall, annotation: &'static NseAnnotation) {
        let Ok(args) = call.arguments() else {
            return;
        };

        // Build a map from argument index (in call order) to the matched
        // ScopedArg, so we can iterate once over the arguments.
        let arg_count = args.items().iter().count();
        let mut arg_scoped: Vec<Option<&'static ScopedArg>> = vec![None; arg_count];
        let mut consumed = vec![false; annotation.scoped_args.len()];

        // Named pass
        for (i, item) in args.items().iter().enumerate() {
            let Ok(arg) = item else { continue };
            if let Some(scoped_idx) = match_named_arg(&arg, annotation, &consumed) {
                consumed[scoped_idx] = true;
                arg_scoped[i] = Some(&annotation.scoped_args[scoped_idx]);
            }
        }

        // Positional pass
        let mut position = 0usize;
        for (i, item) in args.items().iter().enumerate() {
            let Ok(arg) = item else {
                position += 1;
                continue;
            };
            if arg.name_clause().is_some() {
                position += 1;
                continue;
            }
            if arg_scoped[i].is_none() {
                if let Some(scoped_idx) = match_positional_arg(annotation, position, &consumed) {
                    consumed[scoped_idx] = true;
                    arg_scoped[i] = Some(&annotation.scoped_args[scoped_idx]);
                }
            }
            position += 1;
        }

        // Walk arguments, pushing scopes for matched ones
        for (i, item) in args.items().iter().enumerate() {
            let Ok(arg) = item else { continue };
            let Some(value) = arg.value() else { continue };

            if let Some(scoped) = arg_scoped[i] {
                self.collect_nse_argument(&value, scoped);
            } else {
                self.collect_expression(&value);
            }
        }
    }

    /// Walk a single NSE argument body, pushing a scope when appropriate.
    fn collect_nse_argument(&mut self, value: &AnyRExpression, scoped_arg: &ScopedArg) {
        match (scoped_arg.nse_scope, scoped_arg.laziness) {
            (NseScope::Current, ScopeLaziness::Eager) => {
                // No scope push, walk body in place (regular case)
                self.collect_expression(value);
            },
            (nse_scope, laziness) => {
                let kind = ScopeKind::Nse(nse_scope, laziness);
                let scope = self.push_scope(kind, value.syntax().text_trimmed_range());
                self.pre_scan_scope(value.syntax());
                self.collect_expression(value);
                self.pop_scope(scope);
            },
        }
    }

    /// Route a definition from a `Current + Lazy` NSE scope to the parent.
    pub(super) fn add_definition_to_parent(
        &mut self,
        name: &str,
        flags: SymbolFlags,
        kind: DefinitionKind,
        range: TextRange,
    ) {
        let Some(parent_scope) = self.scopes[self.current_scope]
            .parent
            .debug_assert_some("Current-scope NSE has no parent")
        else {
            return;
        };

        let symbol_id = self.symbol_tables[parent_scope].intern(name, flags);
        let def_id = self.definitions[parent_scope].push(Definition {
            symbol: symbol_id,
            kind,
            range,
        });

        let builder = self.use_def_builder_mut(parent_scope);
        builder.ensure_symbol(symbol_id);

        // Deferred: the body executes at an unknown later time, so the
        // definition shouldn't shadow what's already live. This is the same
        // mechanism as `<<-`.
        //
        // Known imprecision: the deferred def is visible to ALL uses in
        // the parent scope (with `may_be_unbound: true`), including
        // file-level uses that run before the lazy body executes. Ideally
        // these defs would only be reachable from lazy scopes (functions),
        // not from eager/file-level code.
        builder.record_deferred_definition(symbol_id, def_id);
    }
}

// --- Argument matching helpers ---

/// Match a named argument against the annotation's scoped args. Returns the
/// index into `annotation.scoped_args` if matched.
fn match_named_arg(
    arg: &aether_syntax::RArgument,
    annotation: &NseAnnotation,
    consumed: &[bool],
) -> Option<usize> {
    let name_clause = arg.name_clause()?;
    let name = name_clause.name().ok()?;
    let name_text = match &name {
        AnyRArgumentName::RIdentifier(ident) => ident.name_text(),
        AnyRArgumentName::RStringValue(s) => s.string_text()?,
        _ => return None,
    };
    annotation
        .scoped_args
        .iter()
        .enumerate()
        .find(|(i, s)| !consumed[*i] && s.name == name_text.as_str())
        .map(|(i, _)| i)
}

/// Match an unnamed argument at `position` against the annotation's scoped
/// args. Returns the index into `annotation.scoped_args` if matched.
fn match_positional_arg(
    annotation: &NseAnnotation,
    position: usize,
    consumed: &[bool],
) -> Option<usize> {
    annotation
        .scoped_args
        .iter()
        .enumerate()
        .find(|(i, s)| !consumed[*i] && s.position == position)
        .map(|(i, _)| i)
}

/// Extract a name from an `AnyRSelector` (the LHS or RHS of `pkg::fn`).
fn selector_name(selector: &AnyRSelector) -> Option<String> {
    match selector {
        AnyRSelector::RIdentifier(ident) => Some(ident.name_text()),
        AnyRSelector::RStringValue(s) => s.string_text(),
        _ => None,
    }
}
