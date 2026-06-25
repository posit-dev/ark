use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::RArgumentList;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use oak_core::syntax_ext::AnyRSelectorExt;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use stdext::debug_panic;

use super::SemanticIndexBuilder;
use crate::effects::Effects;
use crate::effects::NseAnnotation;
use crate::effects::NseArgument;
use crate::effects_registry;
use crate::resolver::ImportsResolver;
use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SymbolId;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    /// Resolve a call's callee to an NSE annotation.
    ///
    /// Two cases resolve here:
    /// - A bare identifier. If the callee is unbound, it is resolved
    ///   through the cross-file `ImportsResolver::resolve_effects()` method.
    ///   If bound locally, we'll resolve the annotations here - TODO(nse, annotations).
    /// - A `pkg::fn` namespace expression, resolved through
    ///   `ImportsResolver::resolve_qualified_effects()`. `::` names the package,
    ///   so there's no search-path disambiguation; the resolver answers from
    ///   per-package knowledge (the static registry, plus cross-file knowledge
    ///   like the re-export chase once that lands).
    pub(super) fn resolve_nse(&mut self, call: &RCall) -> Option<NseAnnotation> {
        let func = call.function().ok()?;

        match &func {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();

                // Bail early if it is known that no package annotates this name
                // with effects. This speeds up the common case of no known annotations.
                if !effects_registry::is_annotated(&name) {
                    return None;
                }

                let Some(symbol_id) = self.symbol_tables[self.current_scope].id(&name) else {
                    debug_panic!(
                        "Callee `{name}` not interned: collect_expression should have run first"
                    );
                    return None;
                };

                // First check for a local definition (which in the future will
                // potentially contain NSE annotations)
                if self.is_locally_bound(&name) {
                    return self
                        .resolve_effects(symbol_id)
                        .and_then(|effects| effects.nse);
                }

                // Now check imports since the symbol is locally unbound
                self.resolver
                    .resolve_effects(&name, &[], false)
                    .and_then(|effects| effects.nse)
            },

            AnyRExpression::RNamespaceExpression(ns_expr) => {
                let left = ns_expr.left().ok()?;
                let right = ns_expr.right().ok()?;
                let pkg = left.identifier_text()?;
                let func_name = right.identifier_text()?;

                if !effects_registry::is_annotated(&func_name) {
                    return None;
                }

                self.resolver
                    .resolve_qualified_effects(&pkg, &func_name)
                    .and_then(|effects| effects.nse)
            },

            _ => None,
        }
    }

    /// Local resolver for declared effects, mirroring the imports revoler's
    /// `resolve_effects()` method on the cross-file side.
    /// TODO(nse, annotations): always `None` until `declare()` parsing lands.
    fn resolve_effects(&self, _symbol_id: SymbolId) -> Option<Effects> {
        None
    }

    /// Whether the current scope or an enclosing one binds `name`, shadowing
    /// the base NSE callee. The current scope is always flow-precise. For
    /// ancestors, crossing a lazy scope (e.g. a function body) loses the
    /// accuracy because we don't know when the lazy scope runs and need to
    /// consider the whole scope bindings, not just the ones currently live.
    ///
    /// Invariant: The eager/lazy decision must match the decision in
    /// `register_enclosing_snapshot()`. If they disagree, a call flips between
    /// NSE and not-NSE across re-walks and the fixpoint never settles.
    fn is_locally_bound(&self, name: &str) -> bool {
        if self.scope_binds_so_far(self.current_scope, name) {
            return true;
        }

        let Some(mut scope) = self.scopes[self.current_scope].parent else {
            return false;
        };
        let mut all_eager = !self.scopes[self.current_scope].kind.is_lazy();

        loop {
            let bound = if all_eager {
                self.scope_binds_so_far(scope, name)
            } else {
                self.scope_binds_anywhere(scope, name)
            };
            if bound {
                return true;
            }

            if self.scopes[scope].kind.is_lazy() {
                all_eager = false;
            }

            let Some(parent) = self.scopes[scope].parent else {
                return false;
            };
            scope = parent;
        }
    }

    /// Process a call already recognized as NSE. Match its arguments against the
    /// annotation, then handle each scoped argument.
    ///
    /// For every scoped argument we record the decision. That sets `found_nse`.
    /// For `Nested` arguments it also notes the body range which allows
    /// pre-scans to skip it.
    ///
    /// How we walk the body depends on the phase. The first walk keeps it flat
    /// (no nested scope), so its definitions land in the current scope. The
    /// re-walk pushes the NSE scope and walks the body inside it.
    pub(super) fn collect_nse_call(&mut self, call: &RCall, annotation: NseAnnotation) {
        let Ok(args) = call.arguments() else {
            return;
        };
        let items = args.items();
        let nse_args = self.match_nse_args(&items, annotation);

        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            let Some(value) = arg.value() else { continue };

            let Some(nse_arg) = nse_args[i] else {
                self.collect_expression(&value);
                continue;
            };

            self.record_nse_argument(nse_arg, &value);

            if self.is_rewalk {
                // On rewalks, we push nested NSE scopes and collect definitions there
                self.collect_nse_argument(nse_arg, &value);
            } else {
                // On the first walk, keep flat and collect definitions in the current scope
                self.collect_expression(&value);
            }
        }
    }

    /// Match a call's arguments against an NSE annotation. Returns, per argument
    /// in call order, the scoped argument it matched (if any). Named arguments
    /// match first, then unmatched positions fill by call-site position.
    ///
    /// FIXME: This is a stopgap helper. In the future, `Effects` will be
    /// returned from the resolvers with the function signature, and we'll
    /// implement a proper argument matching routine.
    fn match_nse_args(
        &self,
        items: &RArgumentList,
        annotation: NseAnnotation,
    ) -> Vec<Option<&'static NseArgument>> {
        let arg_count = items.iter().count();
        let mut nse_args: Vec<Option<&'static NseArgument>> = vec![None; arg_count];
        let mut consumed = vec![false; annotation.arguments.len()];

        // Named pass
        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            if let Some(nse_idx) = match_named_arg(&arg, &annotation, &consumed) {
                consumed[nse_idx] = true;
                nse_args[i] = Some(&annotation.arguments[nse_idx]);
            }
        }

        // Positional pass. Only unnamed args reach the match, and none of them
        // were set by the named pass, so no need to re-check `nse_args[i]`.
        let mut position = 0usize;
        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else {
                position += 1;
                continue;
            };
            if arg.name_clause().is_some() {
                position += 1;
                continue;
            }
            if let Some(scoped_idx) = match_positional_arg(&annotation, position, &consumed) {
                consumed[scoped_idx] = true;
                nse_args[i] = Some(&annotation.arguments[scoped_idx]);
            }
            position += 1;
        }

        nse_args
    }

    fn record_nse_argument(&mut self, scoped: &NseArgument, value: &AnyRExpression) {
        match (scoped.scope, scoped.timing) {
            // Doesn't push a scope, nothing to record.
            (NseScope::Current, NseTiming::Eager) => {},
            // Routes to the parent. No body range to skip, but still a virtual scope.
            (NseScope::Current, NseTiming::Lazy) => {
                self.found_nse = true;
            },
            // Note the body range so pre-scans skip it.
            (NseScope::Nested, _) => {
                self.found_nse = true;
                self.nse_nested_ranges
                    .insert(value.syntax().text_trimmed_range());
            },
        }
    }

    /// Walk a single NSE argument body, pushing a scope when appropriate.
    fn collect_nse_argument(&mut self, nse_arg: &NseArgument, value: &AnyRExpression) {
        match (nse_arg.scope, nse_arg.timing) {
            (NseScope::Current, NseTiming::Eager) => {
                self.collect_expression(value);
            },

            (nse_scope, nse_timing) => {
                let kind = ScopeKind::Nse(nse_scope, nse_timing);
                let scope = self.push_scope(kind, value.syntax().text_trimmed_range());

                // Only `Nested` scopes hold their own definitions and get pre-scanned
                if nse_scope == NseScope::Nested {
                    self.pre_scan_scope(value.syntax());
                }

                self.collect_expression(value);
                self.pop_scope(scope);
            },
        }
    }
}

/// Match a named argument against the annotation's arguments. Returns the
/// index into `annotation.arguments` if matched.
///
/// Should we do partial argument matching? Or rely on partial matching being linted?
fn match_named_arg(
    arg: &aether_syntax::RArgument,
    annotation: &NseAnnotation,
    consumed: &[bool],
) -> Option<usize> {
    let clause = arg.name_clause()?;
    let name = clause.name().ok()?;
    let name_text = match &name {
        AnyRArgumentName::RIdentifier(ident) => ident.name_text(),
        AnyRArgumentName::RStringValue(s) => s.string_text()?,
        _ => return None,
    };
    annotation
        .arguments
        .iter()
        .enumerate()
        .find(|(i, nse_arg)| !consumed[*i] && nse_arg.name == name_text.as_str())
        .map(|(i, _)| i)
}

/// Match an unnamed argument at `position` against the annotation's arguments.
/// Returns the index into `annotation.arguments` if matched.
///
/// FIXME: This matches positionally on call-site position only: an unnamed
/// argument at position N matches an annotation argument declared at position
/// N. It doesn't replicate R's full matching, where named arguments are pulled
/// out first and the rest fill the remaining formals in order. So `test_that({
/// ... }, desc = "d")`, with the block at position 0 but the `code` formal at
/// position 1, won't match. Good enough without the callee's formal list;
/// revisit if it misses real cases.
fn match_positional_arg(
    annotation: &NseAnnotation,
    position: usize,
    consumed: &[bool],
) -> Option<usize> {
    annotation
        .arguments
        .iter()
        .enumerate()
        .find(|(i, scoped)| !consumed[*i] && scoped.position == position)
        .map(|(i, _)| i)
}
