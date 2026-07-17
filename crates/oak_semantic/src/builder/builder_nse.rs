use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::RArgumentList;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstSeparatedList;
use biome_rowan::TextRange;
use oak_core::syntax_ext::AnyRSelectorExt;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use super::assignment_name;
use super::is_assignment;
use super::is_right_assignment;
use super::is_super_assignment;
use super::BoundNames;
use super::SemanticIndexBuilder;
use crate::effects::Argument;
use crate::effects::ArgumentsAnnotation;
use crate::effects::Effects;
use crate::effects_registry;
use crate::resolver::ImportsResolver;
use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticDiagnostic;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    /// Scan a call for effects (e.g. NSE scopes) and record its decision for
    /// the walk to reuse.
    ///
    /// If the callee resolves to an NSE annotation, the annotation is recorded
    /// in `call_resolutions` under the call's range (as the entry's `nse`).
    /// Arguments evaluated in nested calls are scanned accordingly. Otherwise
    /// all arguments are scanned in the current scope.
    ///
    /// `Current + Eager` and `Nested + Eager` arguments are scanned here:
    /// `Current + Eager` transparently, `Nested + Eager` by descending into the
    /// body and holding the names it binds as pending. `Nested + Lazy` and
    /// `Current + Lazy` bodies are their own scan units and deferred to the walk
    /// because resolution of effects in these lazy scopes needs the child's own
    /// flow context.
    pub(super) fn scan_call(&mut self, call: &RCall) {
        let Some(annotation) = self.resolve_nse(call) else {
            if let Ok(args) = call.arguments() {
                for item in args.items().iter() {
                    let Ok(arg) = item else { continue };
                    if let Some(value) = arg.value() {
                        self.scan_expression(&value);
                    }
                }
            }
            return;
        };

        self.call_resolutions
            .entry(call.syntax().text_trimmed_range())
            .or_default()
            .nse = Some(annotation);

        let Ok(args) = call.arguments() else {
            return;
        };
        let items = args.items();
        let nse_args = self.match_nse_arguments(&items, annotation);

        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            let Some(value) = arg.value() else { continue };

            match nse_args[i] {
                None => self.scan_expression(&value),
                Some(nse_arg) => match (nse_arg.scope, nse_arg.timing) {
                    // Calls like `evalq()`
                    (NseScope::Current, NseTiming::Eager) => self.scan_expression(&value),

                    // Calls like `on_load()`. Its body runs later, so its defs
                    // land in the enclosing scope. We don't resolve the body's
                    // calls here. The walk does that once it enters the child
                    // scope. But we do grab the names it defines now, so the
                    // owner's bound names are complete before the walk reaches a sibling.
                    (NseScope::Current, NseTiming::Lazy) => {
                        self.record_inherited_at_entry(value.syntax().text_trimmed_range());
                        self.scan_lazy_owner_bindings(&value);
                    },

                    // Calls like `local()`. Its body runs eagerly at the call
                    // site, so its environment IS the current `bound_so_far`.
                    // Descend now, holding the names bound in this scope as
                    // pending so the walk has access to them. No `bound_so_far`
                    // reset: the child sees exactly what `begin_scan()` would
                    // have seeded.
                    // No `record_inherited_at_entry()`: eager `Nested` bodies are
                    // never scanned at walk time, so nothing would read it.
                    (NseScope::Nested, NseTiming::Eager) => {
                        let old = self.bound_so_far.clone();

                        let range = value.syntax().text_trimmed_range();
                        self.descent.open.push(BoundNames::new());
                        self.scan_expression(&value);
                        if let Some(bound) = self.descent.open.pop() {
                            self.descent.pending.insert(range, bound);
                        }

                        self.bound_so_far = old;
                    },

                    // Calls like `reactive()`. Its body runs at an unknown
                    // later time, so it's a child scope scanned when the walk
                    // enters it. Record the names it inherits for its callee
                    // resolution, same as a function body.
                    (NseScope::Nested, NseTiming::Lazy) => {
                        self.record_inherited_at_entry(value.syntax().text_trimmed_range());
                    },
                },
            }
        }
    }

    /// Copy the names a `Current + Lazy` body defines into the owner's
    /// bound names, without marking them bound in the scan's flow state.
    ///
    /// This feeds enclosing snapshots only. A free variable elsewhere can
    /// resolve to a name an `on_load({ ... })` defines in the owner, and
    /// `register_enclosing_snapshot()` reads `bound_names` to find that ancestor.
    /// The scan doesn't descend into these bodies otherwise, so their names
    /// would only reach the owner when the walk later gets to the call, too late
    /// for a sibling scanned before then. Collecting them now keeps the owner's
    /// bound names complete before the walk touches any sibling.
    ///
    /// NSE shadow resolution does not read `bound_names`, so an incomplete
    /// collection here can't flip an NSE decision. `is_locally_bound` reads the
    /// captured eager bindings, which exclude deferred names by construction.
    ///
    /// We cover the realistic shapes, direct assignments and control flow, e.g.
    /// `on_load({ x <- 1 })`. We stop at nested calls and function bodies
    /// however, so we only add names the walk will also route, never a phantom.
    /// `register_enclosing_snapshot` reads `binds()` as "a real definition
    /// exists", so a phantom would send it chasing a binding that isn't there.
    /// The price is a binding buried in a nested transparent call, e.g.
    /// `on_load({ evalq(helper <- ...) })`, which we miss here, so a free
    /// variable can't resolve to it. TODO(nse): We could potentially walk
    /// transparent (Current) nested calls to collect those too.
    ///
    /// The names go to `bound_names` only, never to `bound_so_far`. The body
    /// runs at some later time, so at an eager position after the call these
    /// names aren't bound yet, and an eager callee there must still treat them
    /// as unbound.
    fn scan_lazy_owner_bindings(&mut self, expr: &AnyRExpression) {
        match expr {
            AnyRExpression::RBracedExpressions(braced) => {
                for expr in braced.expressions().iter() {
                    self.scan_lazy_owner_bindings(&expr);
                }
            },

            AnyRExpression::RBinaryExpression(bin) => {
                // `<<-` binds in an ancestor, not the owner, so it's not routed
                // here (matching `add_definition`).
                if !is_assignment(bin) || is_super_assignment(bin) {
                    return;
                }
                let target = if is_right_assignment(bin) {
                    bin.right()
                } else {
                    bin.left()
                };
                if let Ok(target) = target {
                    if let Some((name, _)) = assignment_name(&target) {
                        self.record_owner_name(name);
                    }
                }
            },

            AnyRExpression::RIfStatement(stmt) => {
                if let Ok(consequence) = stmt.consequence() {
                    self.scan_lazy_owner_bindings(&consequence);
                }
                if let Some(else_clause) = stmt.else_clause() {
                    if let Ok(alternative) = else_clause.alternative() {
                        self.scan_lazy_owner_bindings(&alternative);
                    }
                }
            },

            AnyRExpression::RForStatement(stmt) => {
                if let Ok(variable) = stmt.variable() {
                    self.record_owner_name(variable.name_text());
                }
                if let Ok(body) = stmt.body() {
                    self.scan_lazy_owner_bindings(&body);
                }
            },

            AnyRExpression::RWhileStatement(stmt) => {
                if let Ok(body) = stmt.body() {
                    self.scan_lazy_owner_bindings(&body);
                }
            },

            AnyRExpression::RRepeatStatement(stmt) => {
                if let Ok(body) = stmt.body() {
                    self.scan_lazy_owner_bindings(&body);
                }
            },

            // Stop everywhere else: function bodies are child scopes, and a
            // call's arguments aren't part of this scope's direct level.
            _ => {},
        }
    }

    /// Resolve a call's callee to an NSE annotation.
    ///
    /// Two cases resolve here:
    /// - A bare identifier. If bound locally it goes through the local
    ///   [`resolve_local_effects`](Self::resolve_local_effects). Otherwise the
    ///   cross-file `ImportsResolver::resolve_effects()` resolves it across the
    ///   search path.
    /// - A `pkg::fn` namespace expression, resolved through
    ///   `ImportsResolver::resolve_qualified_effects()`. `::` names the package,
    ///   so there's no search-path disambiguation; the resolver answers from
    ///   per-package knowledge (the static registry, plus cross-file knowledge
    ///   like the re-export chase once that lands).
    ///
    /// The bound check reads the scan pass's flow-precise binding state
    /// for the current scope, so this must run during the scan, not the walk.
    fn resolve_nse(&mut self, call: &RCall) -> Option<ArgumentsAnnotation> {
        let func = call.function().ok()?;

        match &func {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();

                // First check for a local definition (which in the future may
                // contain NSE annotations that we resolve here)
                //
                // Looked up from `bound_so_far` which already carries every
                // eager binding visible here: the scope's own flow-precise
                // bindings so far, plus the enclosing eager environment seeded
                // at `begin_scan()`. Forward and deferred (lazy-routed)
                // bindings are excluded. A forward one isn't in `bound_so_far`
                // yet, and a deferred one (`on_load`, `<<-`) never enters it.
                if self.bound_so_far.contains(&name) {
                    return self
                        .resolve_local_effects(&name)
                        .and_then(|effects| effects.nse);
                }

                // Bail early if it is known that no package annotates this name
                // with effects. This speeds up the common case of no known annotations.
                if !effects_registry::annotates(&name) {
                    return None;
                }

                // Now check imports since the symbol is locally unbound. The
                // arena's `current_scope` is the scan unit's scope (the descent
                // pushes no arena scopes), so its laziness is the "am I in a lazy
                // context" test the resolver needs.
                let lazy = self.scopes[self.current_scope].kind.is_lazy();
                let nse = self
                    .resolver
                    .resolve_effects(&name, &[], lazy)
                    .and_then(|effects| effects.nse)?;

                // The callee is unbound by any eager binding, so it is NSE.
                // If a lazy-crossed ancestor binds it whole-scope, that binding's
                // timing relative to this deferred body is undetermined, so the
                // decision is a guess. Flag it.
                if self.is_lazily_shadowed(&name) {
                    self.record_lazy_shadow_ambiguity(name, call.syntax().text_trimmed_range());
                }
                Some(nse)
            },

            AnyRExpression::RNamespaceExpression(ns_expr) => {
                let left = ns_expr.left().ok()?;
                let right = ns_expr.right().ok()?;
                let pkg = left.identifier_text()?;
                let func_name = right.identifier_text()?;

                if !effects_registry::annotates(&func_name) {
                    return None;
                }

                self.resolver
                    .resolve_qualified_effects(&pkg, &func_name)
                    .and_then(|effects| effects.nse)
            },

            _ => None,
        }
    }

    /// Local resolver for declared effects, mirroring the imports resolver's
    /// `resolve_effects()` method on the cross-file side.
    /// TODO(nse, annotations): always `None` until `declare()` parsing lands.
    fn resolve_local_effects(&self, _name: &str) -> Option<Effects> {
        None
    }

    /// Detect ambiguities caused by laziness.
    ///
    /// We've decided `name` is NSE because it was locally unbound at the
    /// current flow cursor, and eager-flow resolution found an NSE effect. If
    /// we're in a lazy context, that decision could be wrong: an enclosing
    /// scope may bind `name` with a timing we can't pin down, either a later
    /// assignment, or one from another deferred body that could run before or
    /// after us We detect this ambiguity here so it can be linted.
    fn is_lazily_shadowed(&self, name: &str) -> bool {
        let mut scope = self.current_scope;
        let mut crossed_lazy = self.scopes[scope].kind.is_lazy();

        while let Some(parent) = self.scopes[scope].parent {
            if crossed_lazy && self.scope_binds_anywhere(parent, name) {
                return true;
            }

            if self.scopes[parent].kind.is_lazy() {
                crossed_lazy = true;
            }
            scope = parent;
        }

        false
    }

    fn record_lazy_shadow_ambiguity(&mut self, name: String, range: TextRange) {
        self.diagnostics
            .push(SemanticDiagnostic::LazyShadowAmbiguity { name, range });
    }

    /// Process a call the scan pass decided is NSE. Match its arguments
    /// against the annotation, then handle each scoped argument, pushing NSE
    /// scopes inline.
    pub(super) fn collect_nse_call(&mut self, call: &RCall, annotation: ArgumentsAnnotation) {
        let Ok(args) = call.arguments() else {
            return;
        };
        let items = args.items();
        let nse_args = self.match_nse_arguments(&items, annotation);

        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            let Some(value) = arg.value() else { continue };

            match nse_args[i] {
                None => self.collect_expression(&value),
                Some(nse_arg) => self.collect_nse_argument(nse_arg, &value),
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
    fn match_nse_arguments(
        &self,
        items: &RArgumentList,
        annotation: ArgumentsAnnotation,
    ) -> Vec<Option<&'static Argument>> {
        let arg_count = items.iter().count();
        let mut nse_args: Vec<Option<&'static Argument>> = vec![None; arg_count];
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

    /// Walk a single NSE argument body, pushing a scope when appropriate.
    ///
    /// `Current + Eager` stays in the current scope. `Nested + Eager` was
    /// already scanned by the descent, so we install its pending names and only
    /// walk. The remaining lazy bodies are their own scan units that we scan
    /// here on entry.
    fn collect_nse_argument(&mut self, nse_arg: &Argument, value: &AnyRExpression) {
        match (nse_arg.scope, nse_arg.timing) {
            // Calls like `evalq()`
            (NseScope::Current, NseTiming::Eager) => {
                self.collect_expression(value);
            },

            // Calls like `local()`
            (NseScope::Nested, NseTiming::Eager) => {
                let range = value.syntax().text_trimmed_range();
                let kind = ScopeKind::Nse(NseScope::Nested, NseTiming::Eager);
                let scope = self.push_scope(kind, range);

                // Install the pending names the descent recorded for this body,
                // before collecting so lazy children inside can see them via
                // `scope_binds_anywhere()`.
                match self.descent.pending.remove(&range) {
                    Some(bound) => self.bound_names[scope] = bound,
                    None => {
                        // An eager NSE scope is reachable only through the scan
                        // unit that descended into it, so the pending set must
                        // exist. If not this is a builder bug. In release
                        // builds we still scan the body here so the walk can
                        // proceed. This fallback runs with an empty eager
                        // environment and its shadow decisions are more
                        // degraded than a real lazy unit's.
                        stdext::debug_panic!(
                            "Missing pending bound names for eager NSE body at {range:?}"
                        );
                        self.begin_scan();
                        self.scan_expression(value);
                    },
                }

                self.collect_expression(value);
                self.pop_scope(scope);
            },

            (nse_scope, nse_timing) => {
                let kind = ScopeKind::Nse(nse_scope, nse_timing);
                let scope = self.push_scope(kind, value.syntax().text_trimmed_range());

                // Scan the child body before walking it. A `Current + Lazy`
                // scope routes its defs to the owner and holds no bound names of its
                // own, which `record_binding` handles; the scan still runs to
                // record the body's NSE decisions in the child's flow context.
                self.begin_scan();
                self.scan_expression(value);
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
    annotation: &ArgumentsAnnotation,
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
    annotation: &ArgumentsAnnotation,
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
