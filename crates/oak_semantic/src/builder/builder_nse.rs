use aether_syntax::AnyRExpression;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstSeparatedList;
use biome_rowan::TextRange;
use oak_core::syntax_ext::AnyRSelectorExt;
use oak_core::syntax_ext::RIdentifierExt;

use super::assignment_name;
use super::is_assignment;
use super::is_right_assignment;
use super::is_super_assignment;
use super::BoundNames;
use super::SemanticIndexBuilder;
use super::SourcedFile;
use crate::effects::Argument;
use crate::effects::CallContext;
use crate::effects::Effects;
use crate::effects::EffectsHandlers;
use crate::effects::ResolvedArgumentEffects;
use crate::effects_registry;
use crate::resolver::ImportsResolver;
use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticDiagnostic;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    /// Scan a call for effects (NSE scopes, attaches, sources) and record its
    /// decisions for the walk to reuse. The callee is resolved once through
    /// [`resolve_effects`].
    ///
    /// `Current + Eager` and `Nested + Eager` arguments are scanned here:
    /// `Current + Eager` transparently, `Nested + Eager` by descending into the
    /// body and holding the names it binds as pending. `Nested + Lazy` and
    /// `Current + Lazy` bodies are their own scan units and deferred to the walk
    /// because resolution of effects in these lazy scopes needs the child's own
    /// flow context.
    pub(super) fn scan_call(&mut self, call: &RCall) {
        let (nse_args, attach, source) = match self.resolve_effects(call) {
            Some(effects) => (effects.arguments, effects.attach, effects.source),
            None => (None, None, None),
        };

        if let Some(package) = attach {
            self.call_resolutions
                .entry(call.syntax().text_trimmed_range())
                .or_default()
                .attach = Some(package.clone());
            if !self.scopes[self.current_scope].kind.is_lazy() {
                self.attached_flow.push(package);
            }
        }

        // Cache each recognized path with its resolution. The walk reads them
        // back to emit one `Source` semantic call per file. `scan_source_call()`
        // binds the sourced names as it goes so a later callee in this scope
        // can see them.
        if let Some(paths) = source {
            let range = call.syntax().text_trimmed_range();
            for path in paths {
                let resolution = self.scan_source_call(&path, range);
                self.call_resolutions
                    .entry(range)
                    .or_default()
                    .source
                    .push(SourcedFile { path, resolution });
            }
        }

        let Some(nse_args) = nse_args else {
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

        let Ok(args) = call.arguments() else {
            return;
        };
        let items = args.items();

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
                        self.record_enclosing_flow(value.syntax().text_trimmed_range());
                        self.scan_lazy_owner_bindings(&value);
                    },

                    // Calls like `local()`. Its body runs eagerly at the call
                    // site, so its environment IS the current `flow_state`.
                    // Descend now, holding the names bound in this scope as
                    // pending so the walk has access to them. No `flow_state`
                    // reset: the child sees exactly what `begin_scan()` would
                    // have seeded.
                    // No `record_enclosing_flow()`: eager `Nested` bodies are
                    // never scanned at walk time, so nothing would read it.
                    (NseScope::Nested, NseTiming::Eager) => {
                        let old = self.flow_state.snapshot();

                        let range = value.syntax().text_trimmed_range();
                        self.eager_descent.open.push(BoundNames::new());
                        self.scan_expression(&value);
                        if let Some(bound) = self.eager_descent.open.pop() {
                            self.eager_descent.pending.insert(range, bound);
                        }

                        self.flow_state.restore(old);
                    },

                    // Calls like `reactive()`. Its body runs at an unknown
                    // later time, so it's a child scope scanned when the walk
                    // enters it. Record the names it inherits for its callee
                    // resolution, same as a function body.
                    (NseScope::Nested, NseTiming::Lazy) => {
                        self.record_enclosing_flow(value.syntax().text_trimmed_range());
                    },
                },
            }
        }

        // Hand the resolved NSE arguments to the walk (at the end to avoid a clone)
        self.call_resolutions
            .entry(call.syntax().text_trimmed_range())
            .or_default()
            .arguments = Some(nse_args);
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
    /// The names go to `bound_names` only, never to `flow_state`. The body
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
                    if let Some((name, range)) = assignment_name(&target) {
                        self.record_owner_name(name, range);
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
                    self.record_owner_name(
                        variable.name_text(),
                        variable.syntax().text_trimmed_range(),
                    );
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

    /// Resolve a call's effects.
    fn resolve_effects(&mut self, call: &RCall) -> Option<Effects> {
        let handlers = self.resolve_effects_handlers(call)?;

        let ctx = CallContext::new();

        let arguments = handlers
            .arguments
            .and_then(|handler| handler.resolve(call, &ctx));
        let attach = handlers
            .attach
            .and_then(|handler| handler.resolve(call, &ctx));
        let source = handlers
            .source
            .and_then(|handler| handler.resolve(call, &ctx));

        Some(Effects {
            arguments,
            attach,
            source,
        })
    }

    /// Resolve a call's callee to its [`EffectsHandlers`] (NSE, attach, ...).
    ///
    /// The shared core for both NSE recognition ([`scan_call`] reads `.arguments`) and
    /// attach recognition ([`scan_call`] reads `.attach`). Two cases resolve:
    /// - A bare identifier. If bound locally it goes through the local
    ///   [`resolve_local_effects`](Self::resolve_local_effects). Otherwise the
    ///   cross-file `ImportsResolver::resolve_effects()` resolves it across the
    ///   search path, against the attach set in `attached_flow`.
    /// - A `pkg::fn` namespace expression, resolved through
    ///   `ImportsResolver::resolve_qualified_effects()`. `::` names the package,
    ///   so there's no search-path disambiguation; the resolver answers from
    ///   per-package knowledge (the static registry, plus cross-file knowledge
    ///   like the re-export chase once that lands).
    ///
    /// The bound check reads the scan pass's flow-precise binding state
    /// for the current scope, so this must run during the scan, not the walk.
    ///
    /// [`EffectsHandlers`]: crate::effects::EffectsHandlers
    /// [`scan_call`]: Self::scan_call
    fn resolve_effects_handlers(&mut self, call: &RCall) -> Option<EffectsHandlers> {
        let func = call.function().ok()?;

        match &func {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();

                // First check for a local definition (which in the future may
                // carry declared effects that we resolve here)
                //
                // Looked up from `flow_state` which already carries every
                // eager binding visible here: the scope's own flow-precise
                // bindings so far, plus the enclosing eager environment seeded
                // at `begin_scan()`. Forward and deferred (lazy-routed)
                // bindings are excluded. A forward one isn't in `flow_state`
                // yet, and a deferred one (`on_load`, `<<-`) never enters it.
                if self.flow_state.is_bound(&name) {
                    return self.resolve_local_effects(&name);
                }

                // Bail early if it is known that no package annotates this name
                // with effects. This speeds up the common case of no known annotations.
                if !effects_registry::annotates(&name) {
                    return None;
                }

                // Now check imports since the symbol is locally unbound. The
                // arena's `current_scope` is the scan unit's scope (the descent
                // pushes no arena scopes), so its laziness is the "am I in a lazy
                // context" test the resolver needs. `attached_flow` is the
                // flow-precise attach prefix during the file scan and the
                // complete end-of-file set during the walk.
                let lazy = self.scopes[self.current_scope].kind.is_lazy();
                let effects = self
                    .resolver
                    .resolve_effects(&name, &self.attached_flow, lazy)?;

                // The callee is unbound by any eager binding, so its effect
                // holds. If a lazy-crossed ancestor binds it whole-scope, that
                // binding's timing relative to this deferred body is
                // undetermined, so the decision is a guess. Flag it.
                //
                // TODO(diagnostics): a symmetric attach ambiguity is out of
                // scope here. A callee resolved not-effectful could be flipped
                // by an attach from a sibling lazy body (`g <- function()
                // library(shiny); f <- function() reactive({...}`). Detecting it
                // needs the complete set of lazy-context attaches, a post-pass
                // rather than this local ancestor check, so it belongs in the
                // future salsa diagnostics query where this lint should move too.
                if let Some(overwrite_range) = self.is_lazily_shadowed(&name) {
                    self.record_lazy_shadow_ambiguity(
                        name,
                        call.syntax().text_trimmed_range(),
                        overwrite_range,
                    );
                }
                Some(effects)
            },

            AnyRExpression::RNamespaceExpression(ns_expr) => {
                let left = ns_expr.left().ok()?;
                let right = ns_expr.right().ok()?;
                let pkg = left.identifier_text()?;
                let func_name = right.identifier_text()?;

                if !effects_registry::annotates(&func_name) {
                    return None;
                }

                self.resolver.resolve_qualified_effects(&pkg, &func_name)
            },

            _ => None,
        }
    }

    /// Local resolver for declared effects, mirroring the imports resolver's
    /// `resolve_effects()` method on the cross-file side.
    /// TODO(nse, annotations): always `None` until `declare()` parsing lands.
    fn resolve_local_effects(&self, _name: &str) -> Option<EffectsHandlers> {
        None
    }

    /// Detect ambiguities caused by laziness.
    ///
    /// We've recognized an effect for `name` (NSE scope or attach) because it
    /// was locally unbound at the current flow cursor and eager-flow resolution
    /// found one. If we're in a lazy context, that decision could be wrong: an
    /// enclosing scope may bind `name` with a timing we can't pin down, either a
    /// later assignment, or one from another deferred body that could run before
    /// or after us. We detect this ambiguity here so it can be linted.
    ///
    /// Returns the site of the shadowing binding.
    fn is_lazily_shadowed(&self, name: &str) -> Option<TextRange> {
        let mut scope = self.current_scope;
        let mut crossed_lazy = self.scopes[scope].kind.is_lazy();

        while let Some(parent) = self.scopes[scope].parent {
            if crossed_lazy {
                if let Some(range) = self.scope_binding_range(parent, name) {
                    return Some(range);
                }
            }

            if self.scopes[parent].kind.is_lazy() {
                crossed_lazy = true;
            }
            scope = parent;
        }

        None
    }

    fn record_lazy_shadow_ambiguity(
        &mut self,
        name: String,
        call_range: TextRange,
        overwrite_range: TextRange,
    ) {
        self.diagnostics
            .push(SemanticDiagnostic::LazyShadowAmbiguity {
                name,
                call_range,
                overwrite_range,
            });
    }

    /// Process a call the scan pass decided is NSE, using the resolved argument
    /// scoping the scan cached. Handle each scoped argument, pushing NSE scopes
    /// inline.
    pub(super) fn collect_nse_call(&mut self, call: &RCall, nse_args: ResolvedArgumentEffects) {
        let Ok(args) = call.arguments() else {
            return;
        };
        let items = args.items();

        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            let Some(value) = arg.value() else { continue };

            match nse_args[i] {
                None => self.collect_expression(&value),
                Some(nse_arg) => self.collect_nse_argument(nse_arg, &value),
            }
        }
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
                match self.eager_descent.pending.remove(&range) {
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
