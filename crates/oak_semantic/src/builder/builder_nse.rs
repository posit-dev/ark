use aether_syntax::AnyRExpression;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use aether_syntax::RSyntaxKind;
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
use super::DeclaredBinding;
use super::SemanticIndexBuilder;
use super::SourcedFile;
use crate::effects;
use crate::effects::AssignBinding;
use crate::effects::CallContext;
use crate::effects::EffectSite;
use crate::effects::EffectSource;
use crate::effects::Effects;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;
use crate::resolver::ImportsResolver;
use crate::semantic_index::DeclId;
use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticDiagnostic;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    /// Scan a call for effects (NSE scopes, attaches, sources, assigns) and
    /// record its decisions for the walk to reuse. The callee is resolved once
    /// through [`resolve_effects`].
    ///
    /// `Current + Eager` and `Nested + Eager` arguments are scanned here:
    /// `Current + Eager` transparently, `Nested + Eager` by descending into the
    /// body and holding the names it binds as pending. `Nested + Lazy` and
    /// `Current + Lazy` bodies are their own scan units and deferred to the walk
    /// because resolution of effects in these lazy scopes needs the child's own
    /// flow context.
    pub(super) fn scan_call(&mut self, call: &RCall) {
        let (arg_effects, attach, source, assign) = match self.resolve_effects(call) {
            Some(effects) => (
                effects.arguments,
                effects.attach,
                effects.source,
                effects.assign,
            ),
            None => (None, None, None, None),
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
                let resolution = self.scan_source_call(&path);
                self.call_resolutions
                    .entry(range)
                    .or_default()
                    .source
                    .push(SourcedFile { path, resolution });
            }
        }

        // Record each assigned name as a binding so a later callee in this scope
        // sees it shadowed (e.g. `assign("local", identity)` masks base
        // `local`).
        if let Some(bindings) = assign {
            let range = call.syntax().text_trimmed_range();
            for binding in bindings {
                self.record_binding(binding.name.clone(), None);
                self.call_resolutions
                    .entry(range)
                    .or_default()
                    .assign
                    .push(binding);
            }
        }

        let Some(arg_effects) = arg_effects else {
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

            match &arg_effects[i] {
                None => self.scan_expression(&value),
                // Quoted argument: only the unquoted holes are live. Scan these,
                // suppress the rest.
                Some(ResolvedArgumentEffect::Quote { holes }) => {
                    for hole in holes {
                        self.scan_expression(hole);
                    }
                },
                Some(ResolvedArgumentEffect::Nse { scope, timing }) => match (scope, timing) {
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

        // Hand the resolved argument effects to the walk (at the end to avoid a clone)
        self.call_resolutions
            .entry(call.syntax().text_trimmed_range())
            .or_default()
            .arguments = Some(arg_effects);
    }

    /// Scan a binary operator for an assign effect (e.g. magrittr's `x %<>% f()`)
    pub(super) fn scan_operator_assign(&mut self, bin: &RBinaryExpression) {
        let Some(bindings) = self.resolve_operator_assign(bin) else {
            return;
        };
        let range = bin.syntax().text_trimmed_range();
        for binding in bindings {
            self.record_binding(binding.name.clone(), None);
            self.call_resolutions
                .entry(range)
                .or_default()
                .assign
                .push(binding);
        }
    }

    /// Recognize a binding operator (`x %<>% f()`, `x %<~% expr`, `x := expr`)
    /// as an assign effect and build its bindings, or `None` for any other binary
    /// operator.
    fn resolve_operator_assign(&mut self, bin: &RBinaryExpression) -> Option<Vec<AssignBinding>> {
        let op = bin.operator().ok()?;

        // A binding operator is either a `%...%` (`SPECIAL`, e.g. `%<>%`, where
        // the operator text distinguishes it from `%>%`) or the walrus `:=`
        // (`WALRUS`). Gate on the token kind before consulting the registry so we
        // skip the resolver for ordinary operators like `+`.
        if !matches!(op.kind(), RSyntaxKind::SPECIAL | RSyntaxKind::WALRUS) {
            return None;
        }
        let op_text = op.text_trimmed();

        // Bail early if this operator is not known to have effects annotations
        if !effects::annotates(op_text) {
            return None;
        }

        let resolved = self.resolve_symbol_effects(op_text, bin.syntax().text_trimmed_range())?;
        let ctx = CallContext::new();
        match resolved {
            ResolvedEffect::Import(EffectSource::Custom(handler)) => handler
                .resolve(EffectSite::Operator(bin), &ctx)
                .and_then(|effects| effects.assign),
            // A declaration is arg-centric only (call-shaped), so it has nothing
            // to say about an operator site, whether it came from the registry or
            // a local `declare()`.
            ResolvedEffect::Import(EffectSource::Declared(_)) | ResolvedEffect::Local(_) => None,
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
                    if let Some((name, _)) = assignment_name(&target) {
                        self.record_owner_name(name, None);
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
                    self.record_owner_name(variable.name_text(), None);
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
    ///
    /// Lookup and interpretation stay in this one scope, returning owned
    /// [`Effects`], so no `&Declaration` (whether borrowed from the registry or
    /// from the builder's local arena) escapes into the walk.
    fn resolve_effects(&mut self, call: &RCall) -> Option<Effects> {
        let resolved = self.resolve_effects_source(call)?;
        let ctx = CallContext::new();

        match resolved {
            ResolvedEffect::Local(id) => {
                effects::declaration::resolve(&self.declarations[id], call, &ctx)
            },
            ResolvedEffect::Import(EffectSource::Declared(declaration)) => {
                effects::declaration::resolve(declaration, call, &ctx)
            },
            ResolvedEffect::Import(EffectSource::Custom(handler)) => {
                handler.resolve(EffectSite::Call(call), &ctx)
            },
        }
    }

    /// Resolve a call's callee to its [`ResolvedEffect`] (a local declaration or
    /// a registry provider).
    ///
    /// The shared core for both NSE recognition ([`scan_call`] reads `.arguments`) and
    /// attach recognition ([`scan_call`] reads `.attach`). Two cases resolve:
    /// - A bare identifier, resolved through [`resolve_symbol_effects`], which
    ///   consults local bindings (and their `declare()` declarations) before the
    ///   cross-file registry.
    /// - A `pkg::fn` namespace expression, resolved through
    ///   `ImportsResolver::resolve_qualified_effects()`. `::` names the package,
    ///   so there's no search-path disambiguation; the resolver answers from
    ///   per-package knowledge (the static registry, plus cross-file knowledge
    ///   like the re-export chase once that lands).
    ///
    /// The local check reads the scan pass's flow-precise binding state for the
    /// current scope, so this must run during the scan, not the walk.
    ///
    /// [`resolve_symbol_effects`]: Self::resolve_symbol_effects
    /// [`scan_call`]: Self::scan_call
    fn resolve_effects_source(&mut self, call: &RCall) -> Option<ResolvedEffect> {
        let func = call.function().ok()?;

        match &func {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();
                self.resolve_symbol_effects(&name, call.syntax().text_trimmed_range())
            },

            AnyRExpression::RNamespaceExpression(ns_expr) => {
                let left = ns_expr.left().ok()?;
                let right = ns_expr.right().ok()?;
                let pkg = left.identifier_text()?;
                let func_name = right.identifier_text()?;

                if !effects::annotates(&func_name) {
                    return None;
                }

                self.resolver
                    .resolve_qualified_effects(&pkg, &func_name)
                    .map(ResolvedEffect::Import)
            },

            _ => None,
        }
    }

    /// Resolve a callee `sym` to its [`ResolvedEffect`].
    ///
<<<<<<< HEAD
    /// `range` is the invocation's range, used to anchor a lazy-shadow
    /// diagnostic.
    fn resolve_symbol_effects(&mut self, sym: &str, range: TextRange) -> Option<EffectSource> {
        // First check for a local definition (which in the future may
        // carry declared effects that we resolve here)
        //
        // Looked up from `flow_state` which already carries every
        // eager binding visible here: the scope's own flow-precise
        // bindings so far, plus the enclosing eager environment seeded
        // at `begin_scan()`. Forward and deferred (lazy-routed)
        // bindings are excluded. A forward one isn't in `flow_state`
        // yet, and a deferred one (`on_load`, `<<-`) never enters it.
        if self.flow_state.is_bound(sym) {
            return self.resolve_local_effects(sym);
        }

        // Bail early if it is known that no package annotates this name
        // with effects. This speeds up the common case of no known annotations.
        if !effects::annotates(sym) {
            return None;
        }

        // Now check imports since the symbol is locally unbound. The
        // arena's `current_scope` is the scan unit's scope (the descent
||||||| parent of d73e4d169 (Resolve local `declare()` annotations)
    /// `range` is the invocation's range, used to anchor a lazy-shadow
    /// diagnostic.
    fn resolve_symbol_effects(&mut self, sym: &str, range: TextRange) -> Option<EffectSource> {
        // Bail early if it is known that no package annotates this name
        // with effects. This speeds up the common case of no known annotations.
        if !effects::annotates(sym) {
            return None;
        }

        // First check for a local definition (which in the future may
        // carry declared effects that we resolve here)
        //
        // Looked up from `bound_so_far` which already carries every
        // eager binding visible here: the scope's own flow-precise
        // bindings so far, plus the enclosing eager environment seeded
        // at `begin_scan()`. Forward and deferred (lazy-routed)
        // bindings are excluded. A forward one isn't in `bound_so_far`
        // yet, and a deferred one (`on_load`, `<<-`) never enters it.
        if self.bound_so_far.contains_key(sym) {
            return self.resolve_local_effects(sym);
        }

        // Now check imports since the symbol is locally unbound. The
        // arena's `current_scope` is the scan unit's scope (the descent
=======
    /// `range` is the invocation's range, used to anchor a lazy-shadow or
    /// declared-mixed diagnostic.
    ///
    /// Three tiers, in order. A locally bound name always shadows the registry,
    /// so the local checks run first and a bound name never falls through.
    fn resolve_symbol_effects(&mut self, sym: &str, range: TextRange) -> Option<ResolvedEffect> {
        // 1. Flow-precise local binding. `bound_so_far` carries every eager
        //    binding visible here: this scope's flow-precise prefix plus the
        //    enclosing eager environment seeded at `begin_scan()`. Forward and
        //    deferred (lazy-routed) bindings are excluded, so a name found here
        //    is bound before this point. It shadows every registry provider,
        //    which is today's behavior kept: `Some(id)` resolves the local
        //    declaration, a plain binding (`None`) resolves to nothing.
        if let Some(binding) = self.bound_so_far.get(sym).copied() {
            let id = binding?;
            // A declaration inherited across a lazy boundary can be contradicted
            // by a later rebind in the binding scope (whole-scope `Mixed`), and
            // the lazy body's timing relative to that rebind is unknowable. Lint
            // it; resolution still uses the flow-precise `id` (the linear pass
            // wins, matching the shadow lint). A binding from this scan unit's
            // own flow has no lazy crossing, so `lazy_crossed_binding` never sees
            // it and it never lints.
            if matches!(self.lazy_crossed_binding(sym), Some(DeclaredBinding::Mixed)) {
                self.record_declared_mixed_ambiguity(sym.to_string(), range);
            }
            return Some(ResolvedEffect::Local(id));
        }

        // 2. Whole-scope declaration reachable across a lazy boundary: a lazy
        //    body legitimately forward-referencing a declaring function defined
        //    later in the file. Skip entirely when no `declare()` was seen:
        //    files without one (the overwhelming majority) pay nothing here.
        if !self.declarations.is_empty() {
            match self.lazy_crossed_binding(sym) {
                // Unanimous: every binding of the name carries this declaration,
                // so the forward reference resolves regardless of definition
                // order.
                Some(DeclaredBinding::Declared(id)) => return Some(ResolvedEffect::Local(id)),
                // The bindings disagree and the body's timing relative to them is
                // unknowable, so answer conservatively (no effect) and lint. Not
                // falling through to the registry matches the shadowing rule: the
                // name IS locally bound whole-scope.
                Some(DeclaredBinding::Mixed) => {
                    self.record_declared_mixed_ambiguity(sym.to_string(), range);
                    return None;
                },
                // Plain, or no lazy-crossed ancestor binds it: today's registry
                // path (including the `is_lazily_shadowed` lint below).
                Some(DeclaredBinding::Plain) | None => {},
            }
        }

        // 3. Registry/imports. Bail early if no package annotates this name,
        //    which speeds up the common case of no known annotations.
        if !effects::annotates(sym) {
            return None;
        }

        // The arena's `current_scope` is the scan unit's scope (the descent
>>>>>>> d73e4d169 (Resolve local `declare()` annotations)
        // pushes no arena scopes), so its laziness is the "am I in a lazy
        // context" test the resolver needs. `attached_flow` is the flow-precise
        // attach prefix during the file scan and the complete end-of-file set
        // during the walk.
        let lazy = self.scopes[self.current_scope].kind.is_lazy();
        let effects = self
            .resolver
            .resolve_effects(sym, &self.attached_flow, lazy)?;

        // The callee is unbound by any eager binding, so its effect holds. If a
        // lazy-crossed ancestor binds it whole-scope, that binding's timing
        // relative to this deferred body is undetermined, so the decision is a
        // guess. Flag it.
        //
        // TODO(diagnostics): a symmetric attach ambiguity is out of scope here. A
        // callee resolved not-effectful could be flipped by an attach from a
        // sibling lazy body (`g <- function() library(shiny); f <- function()
        // reactive({...}`). Detecting it needs the complete set of lazy-context
        // attaches, a post-pass rather than this local ancestor check, so it
        // belongs in the future salsa diagnostics query where this lint should
        // move too.
        if self.is_lazily_shadowed(sym) {
            self.record_lazy_shadow_ambiguity(sym.to_string(), range);
        }

        Some(ResolvedEffect::Import(effects))
    }

    /// Detect ambiguities caused by laziness.
    ///
    /// We've recognized an effect for `name` (NSE scope or attach) because it
    /// was locally unbound at the current flow cursor and eager-flow resolution
    /// found one. If we're in a lazy context, that decision could be wrong: an
    /// enclosing scope may bind `name` with a timing we can't pin down, either a
    /// later assignment, or one from another deferred body that could run before
    /// or after us. We detect this ambiguity here so it can be linted.
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

    /// The joined declaration payload of the nearest ancestor that binds `name`
    /// across a lazy boundary, or `None` when none does.
    ///
    /// The same walk as [`is_lazily_shadowed`](Self::is_lazily_shadowed): only
    /// scopes reached after crossing a lazy boundary count, because within an
    /// eager stretch `bound_so_far` is already exact and a later sibling isn't
    /// visible at run time. Reads the whole-scope [`BoundNames`] payload,
    /// defaulting to `Plain` for a binding that carries none (e.g. a parameter,
    /// which the scan doesn't route through `record_owner_name`).
    fn lazy_crossed_binding(&self, name: &str) -> Option<DeclaredBinding> {
        let mut scope = self.current_scope;
        let mut crossed_lazy = self.scopes[scope].kind.is_lazy();

        while let Some(parent) = self.scopes[scope].parent {
            if crossed_lazy && self.scope_binds_anywhere(parent, name) {
                return Some(
                    self.bound_names[parent]
                        .get(name)
                        .unwrap_or(DeclaredBinding::Plain),
                );
            }

            if self.scopes[parent].kind.is_lazy() {
                crossed_lazy = true;
            }
            scope = parent;
        }

        None
    }

    fn record_declared_mixed_ambiguity(&mut self, name: String, range: TextRange) {
        self.diagnostics
            .push(SemanticDiagnostic::DeclaredMixedAmbiguity { name, range });
    }

    /// Process a call the scan pass decided is NSE, using the resolved argument
    /// scoping the scan cached. Handle each scoped argument, pushing NSE scopes
    /// inline.
    pub(super) fn collect_nse_call(&mut self, call: &RCall, arg_effects: ResolvedArgumentEffects) {
        let Ok(args) = call.arguments() else {
            return;
        };
        let items = args.items();

        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            let Some(value) = arg.value() else { continue };

            let Some(argument) = &arg_effects[i] else {
                self.collect_expression(&value);
                continue;
            };
            match argument {
                ResolvedArgumentEffect::Nse { scope, timing } => {
                    self.collect_nse_argument(*scope, *timing, &value)
                },
                // Quoted argument: only the unquote holes are live.
                ResolvedArgumentEffect::Quote { holes } => {
                    for hole in holes {
                        self.collect_expression(hole);
                    }
                },
            }
        }
    }

    /// Walk a single NSE argument body, pushing a scope when appropriate.
    ///
    /// `Current + Eager` stays in the current scope. `Nested + Eager` was
    /// already scanned by the descent, so we install its pending names and only
    /// walk. The remaining lazy bodies are their own scan units that we scan
    /// here on entry.
    fn collect_nse_argument(&mut self, scope: NseScope, timing: NseTiming, value: &AnyRExpression) {
        match (scope, timing) {
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

/// The provider a callee's effects resolve to. Internal to the builder so a
/// local declaration (borrowed from the builder's `declarations` arena) and a
/// registry [`EffectSource`] can share one return type, with the arena borrow
/// staying inside `resolve_effects` rather than escaping into the walk.
enum ResolvedEffect {
    /// A local `declare()` declaration, indexed in the builder's arena.
    Local(DeclId),
    /// A registry provider: a `'static` declaration or a custom handler.
    Import(EffectSource),
}
