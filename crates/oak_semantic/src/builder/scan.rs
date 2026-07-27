//! The scan pass: NSE decisions and bound-name collection in flow order,
//! ahead of the walk. See the module docs on [`super`] for the scan/walk
//! split.

use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use aether_syntax::RExpressionList;
use aether_syntax::RParameters;
use aether_syntax::RSyntaxNode;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstSeparatedList;
use biome_rowan::SyntaxNodeCast;
use biome_rowan::TextRange;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::RIdentifierExt;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use super::assignment_name;
use super::effects::attach_search_path;
use super::is_assignment;
use super::is_right_assignment;
use super::is_super_assignment;
use super::SemanticIndexBuilder;
use crate::effects::AssignBinding;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;
use crate::effects::ScopeContext;
use crate::resolver::ImportsResolver;
use crate::resolver::SourceResolution;
use crate::semantic_index::AmbiguityReason;
use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticDiagnostic;
use crate::semantic_index::SymbolFlags;

// Traversal

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    /// Reset the flow-precise binding state for a fresh scope's scan.
    ///
    /// Seeds it with two things:
    ///
    /// - The names inherited from enclosing scopes, captured when this scope was
    ///   entered (its `BodyScan::Deferred` snapshot). The parent's own scan was
    ///   seeded the same way, so this is transitively complete: it holds every
    ///   eager binding visible from an ancestor at this scope's definition point.
    /// - The scope's own already-bound names. For a function scope that's the
    ///   parameters, recorded just before the scan runs. For file and NSE scopes
    ///   nothing local is bound yet.
    ///
    /// Parameter defaults are a special case: they are scanned before the params
    /// are recorded, so `walk_function` seeds the full formal set by hand
    /// (all formals bind at once in R, so a default sees every parameter name).
    pub(super) fn begin_scan(&mut self) {
        let range = self.scopes[self.current_scope].range;

        match self.scan.body_scans.get(&range) {
            // The file scope has no entry: nothing is inherited, so start clean.
            None => {
                self.scan.bound_so_far.clear();
                self.scan.attached_inherited.clear();
            },
            Some(BodyScan::Deferred {
                bound_so_far,
                attached_inherited,
            }) => {
                let bound_so_far = bound_so_far.clone();
                let attached_inherited = attached_inherited.clone();
                self.scan.bound_so_far.restore(bound_so_far);
                self.scan.attached_inherited = attached_inherited;
            },
            Some(BodyScan::Scanned(_)) => {
                // A body scanned inline by an eager descent is installed by the
                // walk without a re-scan, so `begin_scan()` should never meet one.
                stdext::debug_panic!("`begin_scan()` on an already-scanned body at {range:?}");
                self.scan.bound_so_far.clear();
                self.scan.attached_inherited.clear();
            },
        }

        for (_id, symbol) in self.walk.symbol_tables[self.current_scope].iter() {
            if symbol.flags().contains(SymbolFlags::IS_BOUND) {
                self.scan.bound_so_far.bind(symbol.name().to_string());
            }
        }
    }

    pub(super) fn scan_expression_list(&mut self, list: &RExpressionList) {
        for expr in list.iter() {
            self.scan_expression(&expr);
        }
    }

    /// Scan for NSE calls and collect the scope's bound names, in flow order.
    ///
    /// Runs before the walk of a scope. It decides NSE-ness at each call the
    /// same way the walk's [`is_locally_bound`](Self::is_locally_bound) would,
    /// records the decision in `call_resolutions` for the walk to reuse, and adds
    /// non-skipped definition names to `bound_anywhere`. The bound names must be
    /// complete before the walk descends into any child scope, because a lazy
    /// child body can reference an ancestor def the ancestor's walk hasn't
    /// reached yet.
    ///
    /// A scan unit is the file or a lazy body (function, `Nested + Lazy`,
    /// `Current + Lazy`). Each unit is scanned once. Within a unit the scan
    /// descends through every eager boundary it meets, in flow order:
    ///
    /// - A `Current + Eager` body pushes no scope, so it stays part of this
    ///   scope's direct level and is scanned through transparently.
    /// - A `Nested + Eager` body is descended into with a save/restore of
    ///   `bound_so_far`, and the names it binds are staged as
    ///   `BodyScan::Scanned` for the walk to install without re-scanning.
    /// - A `Current + Lazy` body (`on_load()`) binds into the owner scope but
    ///   runs later, so it is queued and scanned at the end of this unit, once
    ///   the owner's `bound_anywhere` is complete (see `scan_deferred_bodies()`).
    /// - Function and `Nested + Lazy` bodies are their own scan units, scanned
    ///   separately when the walk enters them, because NSE resolution there needs
    ///   the child's own flow context.
    ///
    /// Branch analysis is precise. In `if (c) local <- f else local({ y <- 1
    /// })` the else branch sees an NSE call because `local` is unbound on the
    /// else path, which prevents `y` from leaking into the scope.
    pub(super) fn scan_expression(&mut self, expr: &AnyRExpression) {
        match expr {
            AnyRExpression::RFunctionDefinition(func) => {
                // A function body is a child scope, scanned when it's entered.
                // Record the names it inherits now so that when we later resolve
                // an NSE callee inside the body, we can check whether one of them
                // shadows it (see `record_enclosing_flow()`).
                self.record_enclosing_flow(func.syntax().text_trimmed_range());
            },

            AnyRExpression::RBracedExpressions(braced) => {
                self.scan_expression_list(&braced.expressions());
            },

            AnyRExpression::RBinaryExpression(bin) => {
                if is_assignment(bin) {
                    let right = is_right_assignment(bin);

                    // Value side first, mirroring `walk_assignment`: it may
                    // hold NSE calls or nested defs that flow before the binding.
                    let value = if right { bin.left() } else { bin.right() };
                    if let Ok(value) = value {
                        self.scan_expression(&value);
                    }

                    let target = if right { bin.right() } else { bin.left() };
                    if let Ok(target) = target {
                        match assignment_name(&target) {
                            // `<<-` binds in an ancestor, not here, so it doesn't
                            // shadow a callee in this scope (matching the walk).
                            Some((name, range)) if !is_super_assignment(bin) => {
                                self.record_binding(name, range);
                            },
                            Some(_) => {},
                            // Complex target (`x$foo <- v`): no binding, but the
                            // target may hold NSE calls.
                            None => self.scan_expression(&target),
                        }
                    }
                } else {
                    // A binding operator (`x %<>% f()`) binds its left operand.
                    // Scan the operands as uses first, then record the binding,
                    // so a later callee in this scope sees that name shadowed.
                    // Mirrors the value-then-target order of the `is_assignment`
                    // branch.
                    if let Ok(lhs) = bin.left() {
                        self.scan_expression(&lhs);
                    }
                    if let Ok(rhs) = bin.right() {
                        self.scan_expression(&rhs);
                    }
                    self.scan_operator_assign(bin);
                }
            },

            AnyRExpression::RCall(call) => {
                if let Ok(func) = call.function() {
                    self.scan_expression(&func);
                }
                self.scan_call(call);
            },

            AnyRExpression::RForStatement(stmt) => {
                // The for-variable is always bound (R sets it to NULL for empty
                // sequences), so it binds before the body regardless of flow.
                if let Ok(variable) = stmt.variable() {
                    self.record_binding(
                        variable.name_text(),
                        variable.syntax().text_trimmed_range(),
                    );
                }

                if let Ok(sequence) = stmt.sequence() {
                    self.scan_expression(&sequence);
                }

                // Since the body may not run (empty sequence), a binding or
                // `library()` created in the body isn't on every path, so it
                // doesn't persist past the loop. The snapshot/merge intersects
                // bindings away, and the mark truncates the body's attaches.
                let attach_mark = self.scan.attached_so_far.len();
                let pre_body = self.scan.bound_so_far.snapshot();
                if let Ok(body) = stmt.body() {
                    self.scan_expression(&body);
                }
                self.scan.bound_so_far.merge(pre_body);
                self.scan.attached_so_far.truncate(attach_mark);
            },

            AnyRExpression::RIfStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.scan_expression(&condition);
                }

                self.scan_branch(
                    |this| {
                        if let Ok(consequence) = stmt.consequence() {
                            this.scan_expression(&consequence);
                        }
                    },
                    |this| {
                        if let Some(else_clause) = stmt.else_clause() {
                            if let Ok(alternative) = else_clause.alternative() {
                                this.scan_expression(&alternative);
                            }
                        }
                    },
                );
            },

            AnyRExpression::RWhileStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.scan_expression(&condition);
                }

                // Since the body may not run (condition false on entry), a
                // binding or `library()` created in the body isn't on every
                // path, so it doesn't persist. The snapshot/merge intersects
                // bindings away, and the mark truncates the body's attaches.
                let attach_mark = self.scan.attached_so_far.len();
                let pre_body = self.scan.bound_so_far.snapshot();
                if let Ok(body) = stmt.body() {
                    self.scan_expression(&body);
                }
                self.scan.bound_so_far.merge(pre_body);
                self.scan.attached_so_far.truncate(attach_mark);
            },

            // `repeat` loops, subsets, extractions, parentheses, unary ops, and
            // literals: recurse into child expressions. The body of a `repeat`
            // expression always runs once, so its bindings are definite.
            // Identifiers and dots are leaves with no bindings or calls, so
            // they fall through to a no-op walk.
            _ => {
                self.scan_descendants(expr.syntax());
            },
        }
    }

    fn scan_branch(
        &mut self,
        consequence: impl FnOnce(&mut Self),
        alternative: impl FnOnce(&mut Self),
    ) {
        let pre = self.scan.bound_so_far.snapshot();
        // A `library()` in one branch must not be visible in the sibling
        // branch, so peel each side's attaches off at this mark and re-add only
        // what survives the join.
        let attach_mark = self.scan.attached_so_far.len();

        consequence(self);

        let post = self.scan.bound_so_far.snapshot();
        let consequence_attaches = self.scan.attached_so_far.split_off(attach_mark);
        self.scan.bound_so_far.restore(pre);

        alternative(self);

        self.scan.bound_so_far.merge(post);

        // A package attached on only one branch isn't attached on every path, so
        // it drops at the join, the same as a one-branch binding dropping from
        // `bound_so_far`. Only packages attached on both branches survive.
        let alternative_attaches = self.scan.attached_so_far.split_off(attach_mark);
        for package in consequence_attaches {
            if alternative_attaches.contains(&package) {
                self.scan.attached_so_far.push(package);
            }
        }
    }

    /// Walk descendant nodes of `expr`, scanning the outermost
    /// `AnyRExpression` children. The scan analog of
    /// `walk_descendants`.
    fn scan_descendants(&mut self, node: &RSyntaxNode) {
        let mut preorder = node.preorder();
        preorder.next();

        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            if let Some(expr) = node.cast::<AnyRExpression>() {
                self.scan_expression(&expr);
                preorder.skip_subtree();
            }
        }
    }

    /// Scan a call for effects (NSE scopes, attaches, sources, assigns) and
    /// record its decisions for the walk to reuse. The callee is resolved once
    /// through [`resolve_effects`].
    ///
    /// `Current + Eager` and `Nested + Eager` arguments are scanned here:
    /// `Current + Eager` transparently, `Nested + Eager` by descending into the
    /// body and staging the names it binds. A `Current + Lazy` body (`on_load()`)
    /// is queued and scanned at the end of this scan unit's drain, once the
    /// owner's bindings are complete. A `Nested + Lazy` body (`reactive()`) is
    /// its own scan unit, deferred to the walk because resolution of effects in
    /// that lazy scope needs the child's own flow context.
    fn scan_call(&mut self, call: &RCall) {
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
            let call_range = call.syntax().text_trimmed_range();
            self.scan
                .call_resolutions
                .entry(call_range)
                .or_default()
                .attach = Some(package.clone());
            if !self.scopes[self.current_scope].kind.is_lazy() {
                self.scan
                    .attached_anywhere
                    .push((package.clone(), call_range));
                self.scan.attached_so_far.push(package);
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
                self.scan
                    .call_resolutions
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
                self.record_binding(binding.name.clone(), range);
                self.scan
                    .call_resolutions
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

        // We resolved an effect for a callee that isn't bound on every path. If
        // it's bound on *some* earlier path here (a conditional shadow that
        // dropped out of the eager linear view), the scope-shape decision is
        // ambiguous, so flag it before descending into the body.
        self.record_conditional_shadow_ambiguity(call, &arg_effects);

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
                Some(ResolvedArgumentEffect::EvalQ { env, timing }) => match (env, timing) {
                    // Calls like `evalq()`
                    (EvalEnv::Current, EvalTiming::Eager) => self.scan_expression(&value),

                    // Calls like `on_load()`. Its body runs later and binds
                    // into the owner scope, so we queue it and scan it at the
                    // end of this scan unit, once the owner's lexical
                    // environment is fully known. See `scan_deferred_bodies()`.
                    (EvalEnv::Current, EvalTiming::Lazy) => {
                        let attached_inherited = attach_search_path(
                            &self.scan.attached_inherited,
                            &self.scan.attached_so_far,
                        )
                        .into_owned();
                        self.scan.deferred_bodies.push(DeferredBody {
                            body: value.clone(),
                            bound_so_far: self.scan.bound_so_far.snapshot(),
                            attached_inherited,
                        });
                    },

                    // Calls like `local()`. Its body runs eagerly at the call
                    // site, so its environment IS the current `bound_so_far`.
                    // Descend now, staging the names it binds as `Scanned` so the
                    // walk has access to them. No `bound_so_far` reset: the child
                    // sees exactly what `begin_scan()` would have seeded.
                    // No `record_enclosing_flow()`: eager `Nested` bodies are
                    // never scanned at walk time, so nothing would read it.
                    (EvalEnv::Nested, EvalTiming::Eager) => {
                        let old = self.scan.bound_so_far.snapshot();

                        let range = value.syntax().text_trimmed_range();
                        let watermark = self.scan.deferred_bodies.len();
                        self.scan.open_scopes.push(OpenScope {
                            kind: ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager),
                            bindings: BindingSites::new(),
                        });
                        self.scan_expression(&value);

                        // Drain `on_load`s inside this `local()` before popping
                        // its frame, so their names route to the `local` frame,
                        // not the scope below it.
                        self.scan_deferred_bodies(watermark);

                        if let Some(scope) = self.scan.open_scopes.pop() {
                            self.scan
                                .body_scans
                                .insert(range, BodyScan::Scanned(scope.bindings));
                        }

                        self.scan.bound_so_far.restore(old);
                    },

                    // Calls like `reactive()`. Its body runs at an unknown
                    // later time, so it's a child scope scanned when the walk
                    // enters it. Record the names it inherits for its callee
                    // resolution, same as a function body.
                    (EvalEnv::Nested, EvalTiming::Lazy) => {
                        self.record_enclosing_flow(value.syntax().text_trimmed_range());
                    },
                },
            }
        }

        // Hand the resolved argument effects to the walk (at the end to avoid a clone)
        self.scan
            .call_resolutions
            .entry(call.syntax().text_trimmed_range())
            .or_default()
            .arguments = Some(arg_effects);
    }

    /// Flag an NSE call whose scope only exists on some branches, because a
    /// *conditional* binding earlier in this scope shadows its callee.
    ///
    /// Take `if (cond) local <- identity` then `local({ y <- 1 })`. On the
    /// branch where the shadow held, `local` is the user's own standard
    /// evaluation function. The scope shape ambiguously depends on `cond`,
    /// which we record here.
    ///
    /// A conditional shadow is a binding on some path through this scope but not
    /// on every path, so we test both halves directly: bound somewhere in the
    /// scope, and not bound on every path. A binding on every path is a definite
    /// shadow that suppresses the effect, so that call is a plain local one and
    /// never reaches here.
    ///
    /// Two things narrow what fires. The effect must actually build a scope, so
    /// we skip `evalq()` (the `Current + Eager` quadrant, which builds none).
    /// And we check only the current scan scope, so a conditional shadow in an
    /// enclosing scope slips through. In
    ///
    /// ```r
    /// if (cond) local <- identity
    /// with(d, {
    ///     local({ y <- 1 })
    /// })
    /// ```
    ///
    /// the `local({...})` call sits in the `with()` scope, which never bound
    /// `local`, so nothing flags it even though the shape still depends on
    /// `cond`. The `with()` body must be eager for this to go silent: a lazy
    /// body (a function, `reactive()`) instead trips the lazy-shadow diagnostic,
    /// which resolves against the ancestor union. Reachability constraints will
    /// close the eager gap by matching the binding's guard against the call's,
    /// rather than tracking a coarse per-scope set.
    ///
    /// The bindings of `on_load()` or `on.exit()` are not visible yet because
    /// their bodies are scanned after this call. That lazy-timed shadow is left
    /// to the lazy-shadow diagnostic.
    fn record_conditional_shadow_ambiguity(
        &mut self,
        call: &RCall,
        arg_effects: &ResolvedArgumentEffects,
    ) {
        let creates_scope = arg_effects.iter().flatten().any(|effect| {
            matches!(
                effect,
                ResolvedArgumentEffect::EvalQ { env, timing }
                    if !matches!((*env, *timing), (EvalEnv::Current, EvalTiming::Eager))
            )
        });
        if !creates_scope {
            return;
        }

        let Ok(AnyRExpression::RIdentifier(ident)) = call.function() else {
            return;
        };
        let name = ident.name_text();

        // A conditional shadow is a binding on some path but not all: present in
        // this scope's union (`scan_scope_binding_range()`) yet absent from
        // `bound_so_far`.
        let Some(binding_range) = self.scan_scope_binding_range(&name) else {
            return;
        };
        if self.scan.bound_so_far.is_bound(&name) {
            return;
        }

        let call_range = call.syntax().text_trimmed_range();
        self.diagnostics.push(SemanticDiagnostic::EffectAmbiguity {
            name,
            call_range,
            reason: AmbiguityReason::ConditionalShadow { binding_range },
        });
    }

    /// Scan the `Current + Lazy` bodies queued since `watermark`, now that the
    /// enclosing unit's `bound_anywhere` is complete. Runs with the owner's
    /// frame context still live (the arena `current_scope`, plus any open eager
    /// frame like a `local()` the `on_load` sits in).
    pub(super) fn scan_deferred_bodies(&mut self, watermark: usize) {
        // Take the queued bodies above `watermark` and scan them by value.
        // Scanning a deferred body can enqueue nested `on_load()` bodies, which
        // push past `watermark` again, so loop until the tail is empty.
        // Splitting the tail off leaves the outer units' entries below
        // `watermark` in place, and drops `deferred_bodies` back to `watermark`
        // once drained.
        loop {
            let batch = self.scan.deferred_bodies.split_off(watermark);
            if batch.is_empty() {
                break;
            }

            for DeferredBody {
                body,
                bound_so_far,
                attached_inherited,
            } in batch
            {
                let old = self.scan.bound_so_far.snapshot();
                let old_attached =
                    std::mem::replace(&mut self.scan.attached_inherited, attached_inherited);
                self.scan.bound_so_far.restore(bound_so_far);

                self.scan.open_scopes.push(OpenScope {
                    kind: ScopeKind::Nse(EvalEnv::Current, EvalTiming::Lazy),
                    bindings: BindingSites::new(),
                });

                self.scan_expression(&body);

                self.scan.open_scopes.pop();
                self.scan.bound_so_far.restore(old);
                self.scan.attached_inherited = old_attached;
            }
        }
    }

    /// Scan a binary operator for an assign effect (e.g. magrittr's `x %<>% f()`)
    fn scan_operator_assign(&mut self, bin: &RBinaryExpression) {
        let Some(bindings) = self.resolve_operator_assign(bin) else {
            return;
        };
        let range = bin.syntax().text_trimmed_range();
        for binding in bindings {
            self.record_binding(binding.name.clone(), range);
            self.scan
                .call_resolutions
                .entry(range)
                .or_default()
                .assign
                .push(binding);
        }
    }

    pub(super) fn scan_parameter_defaults(&mut self, params: &RParameters) {
        // Seed `bound_so_far` with every parameter names so a callee inside a
        // default value sees the full formal set
        for param in params.items().iter() {
            let Ok(param) = param else { continue };
            let Ok(name) = param.name() else { continue };
            let text = match &name {
                AnyRParameterName::RIdentifier(ident) => ident.name_text(),
                AnyRParameterName::RDots(_) => String::from("..."),
                AnyRParameterName::RDotDotI(ddi) => ddi.syntax().text_trimmed().to_string(),
            };
            self.scan.bound_so_far.bind(text);
        }

        for param in params.items().iter() {
            let Ok(param) = param else { continue };
            let Some(default) = param.default() else {
                continue;
            };
            if let Ok(value) = default.value() {
                self.scan_expression(&value);
            }
        }
    }

    /// Resolve one sourced `path`, bind the names it brings in, and return its
    /// resolution for the caller to cache.
    ///
    /// The binding is eager: `source()` runs at its position, so the sourced
    /// names are bound afterwards and can shadow a later NSE callee (e.g. a
    /// sourced `local` masking base `local`). Returns `None` when the resolver
    /// can't locate the target.
    ///
    /// [`scan_call`]: Self::scan_call
    fn scan_source_call(
        &mut self,
        path: &str,
        source_range: TextRange,
    ) -> Option<SourceResolution> {
        let resolution = self.resolver.resolve_source(path)?;

        // Sourced names originate in another file, so they have no binding site
        // here. Anchor the overwrite range at the `source()` call instead.
        for name in &resolution.names {
            self.record_binding(name.clone(), source_range);
        }

        // A `source()`-forwarded `library()` attaches at this call's flow
        // position, the same as an attach written here directly. Only in eager
        // context, matching `scan_attach_call`'s `!is_lazy()` gate.
        if !self.scopes[self.current_scope].kind.is_lazy() {
            for pkg in &resolution.packages {
                self.scan
                    .attached_anywhere
                    .push((pkg.clone(), source_range));
                self.scan.attached_so_far.push(pkg.clone());
            }
        }

        Some(resolution)
    }

    /// Whether the current evaluation frame binds `name` (see [`scan_scope`]).
    /// For a scope, delegates to [`scope_binds_anywhere`]. For a `local()`
    /// descent body, the names collected into it so far.
    ///
    /// [`scan_scope`]: Self::scan_scope
    /// [`scope_binds_anywhere`]: Self::scope_binds_anywhere
    fn scan_scope_binds(&self, name: &str) -> bool {
        match self.scan_scope() {
            ScanScope::Open(scope) => scope.binds(name),
            ScanScope::Arena(scope) => self.scope_binds_anywhere(scope, name),
        }
    }

    /// The site where the current evaluation frame binds `name`, matching
    /// what [`scan_scope_binds`](Self::scan_scope_binds) counts as a binding
    /// (so it returns `Some` on exactly the same names).
    fn scan_scope_binding_range(&self, name: &str) -> Option<TextRange> {
        match self.scan_scope() {
            ScanScope::Open(scope) => scope.binding_range(name),
            ScanScope::Arena(scope) => self.scope_binding_range(scope, name),
        }
    }

    fn scan_scope_is_global(&self) -> bool {
        match self.scan_scope() {
            ScanScope::Open(_) => false,
            ScanScope::Arena(scope) => matches!(self.scopes[scope].kind, ScopeKind::File),
        }
    }

    fn scan_scope(&self) -> ScanScope<'_> {
        // The current evaluation scope is the innermost open scope that owns
        // its bindings.
        for scope in self.scan.open_scopes.iter().rev() {
            if scope.kind.owns_bindings() {
                return ScanScope::Open(&scope.bindings);
            }
        }

        // Return the arena's current scope if there is no owning open scope.
        // This arena's scope is always owning because `Current + Lazy` bodies
        // (e.g. `on_load()`) are scanned with their owner set to `current_scope`.
        ScanScope::Arena(self.current_scope)
    }
}

// State management

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    /// Record what a child scope (function body, NSE argument) about to be
    /// created at `range` inherits from its ancestors, to seed the child's scan
    /// in `begin_scan`. Called during the scan, where `bound_so_far` is the
    /// parent's flow-precise state at the child's definition point (already
    /// carrying the parent's own inherited ancestors, so the child inherits
    /// transitively).
    ///
    /// The attaches are captured as the parent's whole search path, so a nested
    /// body inherits a branch-local attach through however many levels of
    /// definition sit between them.
    fn record_enclosing_flow(&mut self, range: TextRange) {
        let attached_inherited =
            attach_search_path(&self.scan.attached_inherited, &self.scan.attached_so_far)
                .into_owned();
        self.scan.body_scans.insert(range, BodyScan::Deferred {
            bound_so_far: self.scan.bound_so_far.snapshot(),
            attached_inherited,
        });
    }

    /// Record a binding in both scan binding views.
    ///
    /// `bound_so_far` always learns the name, so a later eager callee in this
    /// scope sees it shadowed. The name also routes into an owning scope's
    /// `bound_anywhere`, matching `add_definition`'s routing during the walk. It
    /// goes to the innermost open frame that owns its bindings (an eager
    /// `Nested` body like `local()`); a lazy frame owns nothing, so its names
    /// skip past it to the owner below. With no owning frame open, it lands in
    /// the arena `current_scope`, which always owns its bindings at scan time
    /// (the same invariant `scan_scope()` relies on).
    fn record_binding(&mut self, name: String, range: TextRange) {
        self.scan.bound_so_far.bind(name.clone());

        for frame in self.scan.open_scopes.iter_mut().rev() {
            if frame.kind.owns_bindings() {
                frame.bindings.add(name, range);
                return;
            }
        }
        self.scan.bound_anywhere[self.current_scope].add(name, range);
    }
}

/// What the scan resolved a single call to, for the walk to reuse. A call can
/// carry several of these at once.
///
/// - `arguments`: the per-argument evaluation effects the call resolved to,
///   filled in flow order. `None` means no annotated arguments (not NSE today).
/// - `attach`: the package a `library()`/`require()` call attaches, recognized
///   shadow-aware on the resolve path. The walk reads it back to emit a scoped
///   `SemanticCall::Attach`.
/// - `source`: the files a recognized `source()` call brings in, each with its
///   resolution.
/// - `assign`: the bindings `assign()`-like calls create in the current scope.
#[derive(Default)]
pub(super) struct CallResolution {
    pub(super) arguments: Option<ResolvedArgumentEffects>,
    pub(super) attach: Option<String>,
    pub(super) source: Vec<SourcedFile>,
    pub(super) assign: Vec<AssignBinding>,
}

/// A single file a `source()` call brings in: its statically-extracted path and
/// the resolution the scan computed for it (`None` when it didn't resolve).
#[derive(Clone)]
pub(super) struct SourcedFile {
    pub(super) path: String,
    pub(super) resolution: Option<SourceResolution>,
}

/// Backs a [`CallContext`]'s [`ScopeQuery`] with the builder's live scope
/// state, so an effect handler (`substitute`) can query bindings during the
/// scan without reaching into the builder directly.
///
/// [`CallContext`]: crate::effects::CallContext
pub(super) struct ScanBindings<'a, R: ImportsResolver> {
    pub(super) builder: &'a SemanticIndexBuilder<R>,
}

impl<R: ImportsResolver> ScopeContext for ScanBindings<'_, R> {
    fn is_bound(&self, name: &str, inherits: bool) -> bool {
        if inherits {
            // The scan's `bound_so_far` carries the current scope's bindings plus
            // the inherited eager environment seeded at `begin_scan`, so it's
            // the lexical answer.
            return self.builder.scan.bound_so_far.is_bound(name);
        }
        self.builder.scan_scope_binds(name)
    }

    fn is_global(&self) -> bool {
        self.builder.scan_scope_is_global()
    }
}

/// The scan's eager linear binding state: the names bound on every path reaching
/// the current point of the current scan unit. Branch and loop joins
/// intersection-merge, so a binding made on only some paths is visible inside
/// its branch and then dropped at the join.
///
/// It's the scan's own flow state, a coarse variant of the walk's use-def map,
/// which isn't built yet. `is_bound` answers "is this name definitely bound
/// here?" so the scan can tell whether a callee is shadowed and decide whether a
/// call is NSE. A conditional shadow drops out here, so it falls through to the
/// effect and the walk records the ambiguity.
#[derive(Clone, Default)]
pub(super) struct FlowState {
    bound: FxHashSet<String>,
}

impl FlowState {
    /// Whether `name` is bound on every path reaching the current point.
    pub(super) fn is_bound(&self, name: &str) -> bool {
        self.bound.contains(name)
    }

    /// Save the current state, to rewind to or to seed a child scan unit from.
    fn snapshot(&self) -> FlowState {
        self.clone()
    }

    /// Rewind to `snapshot`, dropping any bindings recorded since it was taken.
    fn restore(&mut self, snapshot: FlowState) {
        *self = snapshot;
    }

    /// Intersect another path's state at a branch or loop join: keep only names
    /// bound on both paths, so a binding made on just one path drops out.
    fn merge(&mut self, other: FlowState) {
        self.bound.retain(|name| other.bound.contains(name));
    }

    /// Record `name` as bound from here on this path.
    fn bind(&mut self, name: String) {
        self.bound.insert(name);
    }

    /// Drop all bindings, to start a fresh scan unit (see `begin_scan()`).
    fn clear(&mut self) {
        self.bound.clear();
    }
}

/// A scope the scan has entered but the walk has not yet allocated in the
/// arena. A `local()` descent, or an `on_load()` body during its deferred scan.
/// Innermost last on the `open_scopes` stack.
///
/// The arena scope doesn't exist yet because the walk allocates scopes in
/// preorder, and allocating one mid-scan would break the `Scope::descendants`
/// invariant. So a scope's names stage here on `bindings` while the scan is
/// inside it, keyed by nothing but stack position.
///
/// `record_binding()` routes a binding to the innermost owning frame so names
/// land on the body that owns them. A `local()` descent finishes by moving its
/// `bindings` into `body_scans` as [`BodyScan::Scanned`], keyed by the body's
/// range, its pre-arena identity until the walk pushes its scope. An `on_load()`
/// body owns no names (they route to the owner), so its frame is discarded.
pub(super) struct OpenScope {
    pub(super) kind: ScopeKind,
    pub(super) bindings: BindingSites,
}

/// Scan state for a child body, keyed by the body's text range (the body's
/// identity until the walk pushes its arena scope).
pub(super) enum BodyScan {
    /// A walk-time scan unit (function body, `Nested + Lazy` like `reactive()`).
    /// The walk seeds `begin_scan()` from this snapshot.
    Deferred {
        bound_so_far: FlowState,
        attached_inherited: Vec<String>,
    },
    /// Already scanned inline by an eager `Nested` descent (e.g. `local()`).
    Scanned(BindingSites),
}

/// A `Current + Lazy` body queued at its call site, scanned once its
/// enclosing scan unit finishes when the lexical environment is fully known.
#[derive(Clone)]
pub(super) struct DeferredBody {
    pub(super) body: AnyRExpression,
    /// `bound_so_far` captured at the call site, the body's inherited eager env.
    pub(super) bound_so_far: FlowState,
    /// The attach search path at the call site, the body's inherited attaches.
    pub(super) attached_inherited: Vec<String>,
}

/// All definitions in a scope, collected by the scan pass before the
/// walk. Skips child-scope bodies (nested functions and `Nested` NSE bodies).
///
/// Keeps each name's earliest binding site in scan order. This earliest site is
/// mentioned in the lazy-shadow diagnostics.
pub(super) struct BindingSites {
    by_name: FxHashMap<String, TextRange>,
}

impl BindingSites {
    pub(super) fn new() -> Self {
        Self {
            by_name: FxHashMap::default(),
        }
    }

    pub(super) fn binds(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub(super) fn binding_range(&self, name: &str) -> Option<TextRange> {
        self.by_name.get(name).copied()
    }

    fn add(&mut self, name: String, range: TextRange) {
        self.by_name.entry(name).or_insert(range);
    }
}

/// A scope as the scan sees it. A `local()` body scanned inline has no arena
/// scope yet and its bindings are stored on the [`ScanScope`] `open` stack.
/// Every other scope is materialized in the arena. [`scan_scope`] resolves
/// which one is the current evaluation frame.
///
/// [`scan_scope`]: SemanticIndexBuilder::scan_scope
enum ScanScope<'a> {
    Open(&'a BindingSites),
    Arena(ScopeId),
}

#[cfg(test)]
mod tests {
    use super::FlowState;

    #[test]
    fn test_bind_is_bound() {
        let mut state = FlowState::default();
        state.bind("x".to_string());
        assert!(state.is_bound("x"));
    }

    #[test]
    fn test_merge_one_branch_binding_drops() {
        // `if (c) x <- 1`: the else path is empty, the if path binds `x`, so
        // `x` isn't on every path and drops at the join.
        let mut else_path = FlowState::default();
        let mut if_path = FlowState::default();
        if_path.bind("x".to_string());
        else_path.merge(if_path);
        assert!(!else_path.is_bound("x"));
    }

    #[test]
    fn test_merge_both_branches_binding_persists() {
        // `if (c) x <- 1 else x <- 2`: both paths bind `x`, so it persists.
        let mut else_path = FlowState::default();
        else_path.bind("x".to_string());
        let mut if_path = FlowState::default();
        if_path.bind("x".to_string());
        else_path.merge(if_path);
        assert!(else_path.is_bound("x"));
    }

    #[test]
    fn test_merge_keeps_pre_branch_binding() {
        // `x <- 1; if (c) y <- 1`: `x` is bound before, `y` only in the branch.
        let mut pre = FlowState::default();
        pre.bind("x".to_string());
        let mut if_path = pre.clone();
        if_path.bind("y".to_string());
        let mut else_path = pre;
        else_path.merge(if_path);
        assert!(else_path.is_bound("x"));
        assert!(!else_path.is_bound("y"));
    }

    #[test]
    fn test_merge_loop_body_binding_drops() {
        // `for (i in xs) z <- 1`: `i` is bound before the body, `z` only inside,
        // so `z` drops when the body merges with the pre-loop state.
        let mut pre_body = FlowState::default();
        pre_body.bind("i".to_string());
        let mut post_body = pre_body.clone();
        post_body.bind("z".to_string());
        post_body.merge(pre_body);
        assert!(post_body.is_bound("i"));
        assert!(!post_body.is_bound("z"));
    }
}
