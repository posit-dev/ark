//! The walk pass: the recursive descent that writes the arenas (scopes,
//! symbols, definitions, uses, use-def maps), reusing the scan's decisions.
//! See the module docs on [`super`] for the scan/walk split.

use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::RArgumentList;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use aether_syntax::RExpressionList;
use aether_syntax::RFunctionDefinition;
use aether_syntax::RNamespaceExpression;
use aether_syntax::RParameter;
use aether_syntax::RParameters;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstPtr;
use biome_rowan::AstSeparatedList;
use biome_rowan::SyntaxNodeCast;
use biome_rowan::TextRange;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::AnyRSelectorExt;
use oak_core::syntax_ext::RIdentifierExt;

use super::assignment_name;
use super::is_assignment;
use super::is_right_assignment;
use super::is_super_assignment;
use super::scan::SourcedFile;
use super::SemanticIndexBuilder;
use crate::effects::AssignBinding;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;
use crate::resolver::ImportsResolver;
use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionKind;
use crate::semantic_index::EnclosingSnapshotKey;
use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;
use crate::semantic_index::NamespaceAccess;
use crate::semantic_index::NamespaceAccessKind;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticCall;
use crate::semantic_index::SemanticCallKind;
use crate::semantic_index::SymbolFlags;
use crate::semantic_index::SymbolId;
use crate::semantic_index::Use;
use crate::semantic_index::UseId;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    fn add_definition(
        &mut self,
        name: &str,
        flags: SymbolFlags,
        kind: DefinitionKind,
        range: TextRange,
    ) {
        // `Nse(Current, Lazy)` scopes don't own any definitions. We add the
        // definitions to the real enclosing owner scope. Note that `Current +
        // Eager` never reaches here because it doesn't push a scope.
        if matches!(
            self.scopes[self.current_scope].kind,
            ScopeKind::Nse(EvalEnv::Current, EvalTiming::Lazy)
        ) {
            self.add_definition_to_owner(name, flags, kind, range);
            return;
        }

        let symbol_id = self.walk.symbol_tables[self.current_scope].intern(name, flags);
        let def_id = self.walk.definitions[self.current_scope].push(Definition {
            symbol: symbol_id,
            kind,
            range,
        });
        self.walk.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
        self.walk.use_def_maps[self.current_scope].record_definition(symbol_id, def_id);
    }

    /// Route a definition from a `Current + Lazy` scope to the scope that
    /// owns it. That's the nearest ancestor scope which holds its own
    /// definitions. A chain of `Current + Lazy` scopes (e.g. `on_load()` nested
    /// in `on_load()`) is skipped: each one routes to its own owner, so they
    /// all land in the same outer scope.
    fn add_definition_to_owner(
        &mut self,
        name: &str,
        flags: SymbolFlags,
        kind: DefinitionKind,
        range: TextRange,
    ) {
        let Some(target_scope) = self.definition_owner() else {
            stdext::debug_panic!("Current + Lazy scope has no parent");
            return;
        };

        let symbol_id = self.walk.symbol_tables[target_scope].intern(name, flags);
        let def_id = self.walk.definitions[target_scope].push(Definition {
            symbol: symbol_id,
            kind,
            range,
        });

        self.walk.use_def_maps[target_scope].ensure_symbol(symbol_id);

        // Deferred: the body executes at an unknown later time, so the
        // definition shouldn't shadow what's already live. This is the same
        // mechanism as `<<-`.
        //
        // Known imprecision: the deferred def is visible to ALL uses in
        // the parent scope (with `may_be_unbound: true`), including
        // file-level uses that run before the lazy body executes. Ideally
        // these defs would only be reachable from lazy scopes (functions),
        // not from eager/file-level code.
        self.walk.use_def_maps[target_scope].record_deferred_definition(symbol_id, def_id);
    }

    // Super-assignment is lexically in the current scope but binds in an
    // ancestor. We record the definition in the current scope and append
    // it to the target scope's use-def map (without shadowing prior
    // definitions).
    //
    // R's `<<-` walks up the environment chain from the parent, targeting
    // the first scope where the symbol is already bound. If no binding is
    // found, it assigns in the global (file) scope.
    fn add_super_definition(&mut self, name: &str, kind: DefinitionKind, range: TextRange) {
        let Some(parent) = self.scopes[self.current_scope].parent else {
            // A top-level `<<-` has no enclosing frame to walk to, so it binds
            // in the file scope it already sits in. The marker scope and the
            // binding scope coincide, so record one definition carrying both
            // flags rather than pushing two coinciding entries.
            let symbol_id = self.walk.symbol_tables[self.current_scope].intern(
                name,
                SymbolFlags::IS_SUPER_BOUND.union(SymbolFlags::IS_BOUND),
            );
            let def_id = self.walk.definitions[self.current_scope].push(Definition {
                symbol: symbol_id,
                kind,
                range,
            });
            self.walk.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
            self.walk.use_def_maps[self.current_scope]
                .record_deferred_definition(symbol_id, def_id);
            return;
        };

        let target_scope = self.resolve_super_target(name, parent);

        let symbol_id =
            self.walk.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_SUPER_BOUND);
        self.walk.definitions[self.current_scope].push(Definition {
            symbol: symbol_id,
            kind: kind.clone(),
            range,
        });

        let target_symbol =
            self.walk.symbol_tables[target_scope].intern(name, SymbolFlags::IS_BOUND);
        let target_def_id = self.walk.definitions[target_scope].push(Definition {
            symbol: target_symbol,
            kind,
            range,
        });
        self.walk.use_def_maps[target_scope].ensure_symbol(target_symbol);
        self.walk.use_def_maps[target_scope]
            .record_deferred_definition(target_symbol, target_def_id);
    }

    // Walk up from `start` to the first scope where `name` already has
    // `IS_BOUND`. Returns that scope, or the file scope if no binding is found
    // (mirroring R's assignment to the global environment). Reaching the file
    // scope unbound ends the walk there, so its `parent` of `None` is the
    // natural terminator.
    fn resolve_super_target(&self, name: &str, start: ScopeId) -> ScopeId {
        let mut scope = start;
        loop {
            if let Some(id) = self.walk.symbol_tables[scope].id(name) {
                if self.walk.symbol_tables[scope]
                    .symbol(id)
                    .flags()
                    .contains(SymbolFlags::IS_BOUND)
                {
                    return scope;
                }
            }
            let Some(parent) = self.scopes[scope].parent else {
                return scope;
            };
            scope = parent;
        }
    }

    fn add_use(&mut self, name: &str, range: TextRange) {
        let symbol_id =
            self.walk.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_USED);
        let use_id = self.walk.uses[self.current_scope].push(Use {
            symbol: symbol_id,
            range,
        });
        self.walk.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
        self.walk.use_def_maps[self.current_scope].record_use(symbol_id, use_id);

        // Associate free variables with the enclosing snapshot where the
        // variable is defined
        if self.walk.use_def_maps[self.current_scope].is_may_be_unbound(symbol_id) {
            self.register_enclosing_snapshot(name, symbol_id, use_id);
        }
    }

    fn register_enclosing_snapshot(&mut self, name: &str, nested_symbol: SymbolId, use_id: UseId) {
        // We're looking for a parent definition for this scope's free variable
        // so start from parent
        let Some(mut current_scope) = self.scopes[self.current_scope].parent else {
            return;
        };

        // Eager vs lazy snapshot for this free variable. Eager snapshots are
        // precise, lazy ones over-approximate. For instance in:
        //
        // ```
        // x <- 1
        // local({ x })
        // x <- 2
        // ```
        //
        // The eager body in `local()` captures `x <- 1` but not `x <- 2`. If
        // the body was inside a lazy context like `function()` instead, the use
        // of `x` could run at any time and we'd fall back to the accumulated
        // union `{1, 2}`, which is an over-approximation.
        //
        // A precise enclosing snapshot requires eagerness throughout, which we
        // track with `all_eager`.
        let mut all_eager = !self.scopes[self.current_scope].kind.is_lazy();

        loop {
            if self.scope_binds_anywhere(current_scope, name) {
                // Intern with empty flags: we just need a stable `SymbolId` for
                // the lookup key. If the symbol was found via its `IS_BOUND`
                // flag, it already exists. If found via pre-scan only, the later
                // `add_definition()` call during the full walk will set `IS_BOUND`.
                let enclosing_symbol_id =
                    self.walk.symbol_tables[current_scope].intern(name, SymbolFlags::empty());
                self.walk.use_def_maps[current_scope].ensure_symbol(enclosing_symbol_id);

                let entry = if all_eager {
                    // Eager: a fresh point-in-time snapshot per use, no dedup and
                    // no watcher. Two uses at different points in the body can
                    // capture different enclosing states (e.g. either side of a
                    // `<<-`), so they can't share.
                    let snapshot_id = self.walk.use_def_maps[current_scope]
                        .register_eager_snapshot(enclosing_symbol_id);
                    (current_scope, snapshot_id)
                } else {
                    // Lazy: every use of this symbol resolves to the same
                    // growing snapshot, so dedup on (nested scope, nested symbol)
                    // and reuse it across uses.
                    let dedup_key = (self.current_scope, nested_symbol);

                    if let Some(&entry) = self.walk.lazy_snapshots.get(&dedup_key) {
                        entry
                    } else {
                        let snapshot_id = self.walk.use_def_maps[current_scope]
                            .register_lazy_snapshot(enclosing_symbol_id);
                        let entry = (current_scope, snapshot_id);
                        self.walk.lazy_snapshots.insert(dedup_key, entry);
                        entry
                    }
                };

                let use_key = EnclosingSnapshotKey {
                    nested_scope: self.current_scope,
                    nested_use: use_id,
                };
                self.walk.enclosing_snapshots.insert(use_key, entry);

                return;
            }

            if self.scopes[current_scope].kind.is_lazy() {
                all_eager = false;
            }

            let Some(parent) = self.scopes[current_scope].parent else {
                return;
            };
            current_scope = parent;
        }
    }

    fn nse_effect(&self, call: &RCall) -> Option<ResolvedArgumentEffects> {
        self.scan
            .call_resolutions
            .get(&call.syntax().text_trimmed_range())
            .and_then(|resolution| resolution.arguments.clone())
    }

    // --- Recursive descent ---

    pub(super) fn collect_expression_list(&mut self, list: &RExpressionList) {
        for expr in list.iter() {
            self.collect_expression(&expr);
        }
    }

    fn collect_expression(&mut self, expr: &AnyRExpression) {
        match expr {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();
                let range = ident.syntax().text_trimmed_range();
                self.add_use(&name, range);
            },

            AnyRExpression::RDots(dots) => {
                self.add_use("...", dots.syntax().text_trimmed_range());
            },

            AnyRExpression::RDotDotI(ddi) => {
                let name = ddi.syntax().text_trimmed().to_string();
                self.add_use(&name, ddi.syntax().text_trimmed_range());
            },

            AnyRExpression::RFunctionDefinition(func) => {
                self.collect_function(func);
            },

            AnyRExpression::RBracedExpressions(braced) => {
                self.collect_expression_list(&braced.expressions());
            },

            AnyRExpression::RBinaryExpression(bin) => {
                // `<-`, `=`, `->`, `<<-`, and `->>` are assignments when they appear as
                // `RBinaryExpression`. In call arguments, `=` is consumed by
                // the parser into `RArgumentNameClause` instead, so it never
                // reaches here.
                if is_assignment(bin) {
                    self.collect_assignment(bin);
                } else {
                    let range = bin.syntax().text_trimmed_range();
                    let is_binding_op = self
                        .scan
                        .call_resolutions
                        .get(&range)
                        .is_some_and(|resolution| !resolution.assign.is_empty());

                    // A binding operator (`x := expr`) treats its left operand
                    // as a definition target, not a read, the same as `<-`. An
                    // ordinary operator (`a + b`) reads both operands.
                    //
                    // TODO(nse): `%<>%` is compound (`x <- f(x)`), so it also
                    // reads its target. Record that read once the registry
                    // carries a per-operator "reads target" flag.
                    if !is_binding_op {
                        if let Ok(lhs) = bin.left() {
                            self.collect_expression(&lhs);
                        }
                    }
                    if let Ok(rhs) = bin.right() {
                        self.collect_expression(&rhs);
                    }
                    // A `%...%` operator the scan recognized as an assign effect
                    // emits its binding here, after the operand uses.
                    self.collect_assign_operator(bin);
                }
            },

            // Calls and subsets need explicit handling because argument name
            // clauses contain `RIdentifier` nodes that should not be recorded
            // as uses.
            AnyRExpression::RCall(call) => {
                // Record the callee as a use (a no-op for `pkg::fn`) before
                // handling NSE.
                if let Ok(func) = call.function() {
                    self.collect_expression(&func);
                }

                if let Some(scoping) = self.nse_effect(call) {
                    self.collect_nse_call(call, scoping)
                } else if let Ok(args) = call.arguments() {
                    self.collect_arguments(&args.items());
                }

                self.collect_semantic_call(call);
            },
            AnyRExpression::RSubset(subset) => {
                if let Ok(object) = subset.function() {
                    self.collect_expression(&object);
                }
                if let Ok(args) = subset.arguments() {
                    self.collect_arguments(&args.items());
                }
            },
            AnyRExpression::RSubset2(subset) => {
                if let Ok(object) = subset.function() {
                    self.collect_expression(&object);
                }
                if let Ok(args) = subset.arguments() {
                    self.collect_arguments(&args.items());
                }
            },

            AnyRExpression::RExtractExpression(extract) => {
                // For `x$name` or `x@slot`, collect the object and skip the member
                if let Ok(object) = extract.left() {
                    self.collect_expression(&object);
                }
            },

            AnyRExpression::RNamespaceExpression(expr) => {
                self.collect_namespace_access(expr);
            },

            AnyRExpression::RForStatement(stmt) => {
                // The for variable is always bound (R sets it to NULL for
                // empty sequences), so its binding is recorded before the
                // snapshot. Assignments inside the body are conditional
                // (body may not execute for empty sequences).
                if let Ok(variable) = stmt.variable() {
                    self.add_definition(
                        &variable.name_text(),
                        SymbolFlags::IS_BOUND,
                        DefinitionKind::ForVariable(AstPtr::new(stmt)),
                        variable.syntax().text_trimmed_range(),
                    );
                }
                if let Ok(sequence) = stmt.sequence() {
                    self.collect_expression(&sequence);
                }

                let pre_loop = self.walk.use_def_maps[self.current_scope].snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.walk.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.walk.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.walk.uses[self.current_scope],
                    );
                }

                self.walk.use_def_maps[self.current_scope].merge(pre_loop);
            },

            AnyRExpression::RIfStatement(stmt) => {
                // Condition is always evaluated
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_if = self.walk.use_def_maps[self.current_scope].snapshot();

                // If-body (consequence)
                if let Ok(consequence) = stmt.consequence() {
                    self.collect_expression(&consequence);
                }

                let post_if = self.walk.use_def_maps[self.current_scope].snapshot();
                self.walk.use_def_maps[self.current_scope].restore(pre_if);

                // Else-body (alternative), if present. If absent, the
                // "else path" is just the pre-if state we restored to.
                if let Some(else_clause) = stmt.else_clause() {
                    if let Ok(alternative) = else_clause.alternative() {
                        self.collect_expression(&alternative);
                    }
                }

                // After: definitions from both branches are live
                self.walk.use_def_maps[self.current_scope].merge(post_if);
            },

            AnyRExpression::RWhileStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_loop = self.walk.use_def_maps[self.current_scope].snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.walk.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.walk.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.walk.uses[self.current_scope],
                    );
                }

                // Body may not execute
                self.walk.use_def_maps[self.current_scope].merge(pre_loop);
            },

            AnyRExpression::RRepeatStatement(stmt) => {
                // Body always executes at least once, so no merge with pre-loop state.
                if let Ok(body) = stmt.body() {
                    let pre_loop = self.walk.use_def_maps[self.current_scope].snapshot();
                    let first_use = self.walk.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.walk.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.walk.uses[self.current_scope],
                    );
                }
            },

            AnyRExpression::RBogusExpression(_) => {},

            // Generic fallback: walk over descendant nodes and collect their
            // `AnyRExpression` children, letting `collect_expression`
            // handle their contents. This covers `RUnaryExpression`,
            // `RParenthesizedExpression`, `RReturnExpression`, literals, and
            // any future expression types without needing explicit arms.
            //
            // NOTE: This also means that identifiers and assignments inside
            // quoting constructs (`~`, `quote()`, `bquote()`) are recorded as
            // uses and bindings. Refining this requires special-casing these
            // forms, which we defer as future work.
            //
            // Once quoting is handled, `declare()` and `~declare()` will need
            // explicit treatment: its arguments are quoted (not evaluated) but
            // should still be inspected for directives like `source()`.
            // Currently this works by accident because the generic traversal is
            // transparent to both `declare()` and `~`.
            _ => {
                self.collect_descendants(expr.syntax());
            },
        }
    }

    // Walk descendant nodes of `expr`, collecting the outermost
    // `AnyRExpression` nodes and recursing into them via `collect_expression`.
    // This skips intermediate wrapper nodes (e.g. `RElseClause`) while
    // correctly stopping at expression boundaries.
    fn collect_descendants(&mut self, node: &RSyntaxNode) {
        let mut preorder = node.preorder();

        // Skip the root node itself
        preorder.next();

        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            if let Some(expr) = node.cast::<AnyRExpression>() {
                self.collect_expression(&expr);
                preorder.skip_subtree();
            }
        }
    }

    fn collect_function(&mut self, fun: &RFunctionDefinition) {
        let scope = self.push_scope(ScopeKind::Function, fun.syntax().text_trimmed_range());

        if let Ok(params) = fun.parameters() {
            // Scan the default values before collecting them. R binds all
            // formals into the frame at once, so a default sees every parameter
            // name regardless of position: `function(local, b = local(...))` is
            // not NSE. So we seed the whole formal set into `flow_state`
            // up front rather than flow-ordered, then scan each default.
            self.begin_scan();
            self.scan_parameter_defaults(&params);

            // `collect_parameters` adds the parameter definitions and walks
            // each default in source order, finding the NSE decisions the scan
            // above recorded.
            self.collect_parameters(&params);
        }
        if let Ok(body) = fun.body() {
            self.begin_scan();
            self.scan_expression(&body);
            self.collect_expression(&body);
        }

        self.pop_scope(scope);
    }

    fn collect_parameters(&mut self, params: &RParameters) {
        for param in params.items().iter() {
            let Ok(param) = param else { continue };
            self.collect_parameter(&param);
        }
    }

    fn collect_parameter(&mut self, param: &RParameter) {
        let flags = SymbolFlags::IS_BOUND.union(SymbolFlags::IS_PARAMETER);

        if let Ok(name) = param.name() {
            match &name {
                AnyRParameterName::RIdentifier(ident) => {
                    self.add_definition(
                        &ident.name_text(),
                        flags,
                        DefinitionKind::Parameter(AstPtr::new(param)),
                        ident.syntax().text_trimmed_range(),
                    );
                },
                AnyRParameterName::RDots(dots) => {
                    self.add_definition(
                        "...",
                        flags,
                        DefinitionKind::Parameter(AstPtr::new(param)),
                        dots.syntax().text_trimmed_range(),
                    );
                },
                AnyRParameterName::RDotDotI(ddi) => {
                    self.add_definition(
                        &ddi.syntax().text_trimmed().to_string(),
                        flags,
                        DefinitionKind::Parameter(AstPtr::new(param)),
                        ddi.syntax().text_trimmed_range(),
                    );
                },
            }
        }

        if let Some(default) = param.default() {
            if let Ok(value) = default.value() {
                self.collect_expression(&value);
            }
        }
    }

    fn collect_assignment(&mut self, op: &RBinaryExpression) {
        let right = is_right_assignment(op);
        let super_assign = is_super_assignment(op);

        // Value side first to record uses before the binding. The uses
        // might refer to the same symbol as the new binding, but refer
        // to a different place (previous binding).
        let value = if right { op.left() } else { op.right() };
        if let Ok(value) = value {
            self.collect_expression(&value);
        }

        let target = if right { op.right() } else { op.left() };
        let Ok(target) = target else { return };

        let Some((name, range)) = assignment_name(&target) else {
            // Complex target (`x$foo <- rhs`, `x[1] <- rhs`, etc.) does
            // not represent a binding. We recurse for uses.
            self.collect_expression(&target);
            return;
        };

        if super_assign {
            self.add_super_definition(
                &name,
                DefinitionKind::SuperAssignment(AstPtr::new(op)),
                range,
            );
        } else {
            self.add_definition(
                &name,
                SymbolFlags::IS_BOUND,
                DefinitionKind::Assignment(AstPtr::new(op)),
                range,
            );
        }
    }

    fn collect_arguments(&mut self, args: &RArgumentList) {
        for item in args.iter() {
            let Ok(arg) = item else { continue };
            if let Some(value) = arg.value() {
                self.collect_expression(&value);
            }
        }
    }

    fn collect_namespace_access(&mut self, expr: &RNamespaceExpression) {
        let Ok(operator) = expr.operator() else {
            return;
        };
        let kind = match operator.kind() {
            RSyntaxKind::COLON2 => NamespaceAccessKind::Export,
            RSyntaxKind::COLON3 => NamespaceAccessKind::Internal,
            _ => return,
        };
        let Some(package) = expr
            .left()
            .ok()
            .and_then(|selector| selector.identifier_text())
        else {
            return;
        };
        let Some(symbol) = expr
            .right()
            .ok()
            .and_then(|selector| selector.identifier_text())
        else {
            return;
        };
        let offset = expr.syntax().text_trimmed_range().start();
        self.walk
            .namespace_accesses
            .push(NamespaceAccess::new(package, symbol, kind, offset));
    }

    fn collect_semantic_call(&mut self, call: &aether_syntax::RCall) {
        // Attach: the scan recognized it (shadow- and mask-aware) and recorded
        // the package by range. We emit the `SemanticCall::Attach` here so it
        // carries the walk-time scope, e.g. the pushed NSE scope for a
        // `library()` inside `local({...})`.
        let range = call.syntax().text_trimmed_range();
        if let Some(package) = self
            .scan
            .call_resolutions
            .get(&range)
            .and_then(|resolution| resolution.attach.clone())
        {
            self.record_attach(call, package);
        }

        // Source: the scan recognized it (shadow- and mask-aware) on the resolve
        // path and cached the sourced files by range. Their presence is the
        // recognition marker, so we dispatch on it rather than the callee name.
        if self
            .scan
            .call_resolutions
            .get(&range)
            .is_some_and(|resolution| !resolution.source.is_empty())
        {
            self.collect_source_call(call);
        }

        // Assign: same recognition path. The scan cached the bound names and we
        // emit the corresponding definitions so they feed the use-def map,
        // `exports()`, and goto.
        if self
            .scan
            .call_resolutions
            .get(&range)
            .is_some_and(|resolution| !resolution.assign.is_empty())
        {
            self.collect_assign_call(call);
        }
    }

    // ## `library()` / `require()` scoping
    //
    // In R, `library()` always modifies the global search path regardless
    // of where it's called. Statically, we scope the call to
    // `self.current_scope`: at file scope it's visible everywhere (sequential
    // execution is guaranteed), but inside a function it's only visible
    // within that function and its children, since the function might never
    // be called. Same reasoning as `source()` calls.
    fn record_attach(&mut self, call: &RCall, package: String) {
        let call_offset = call.syntax().text_trimmed_range().start();
        self.walk.semantic_calls.push(SemanticCall {
            kind: SemanticCallKind::Attach { package },
            offset: call_offset,
            scope: self.current_scope,
        });
    }

    // ## `source()` resolution
    //
    // `source("file.R")` creates `DefinitionKind::Import` forwarding
    // bindings in the current scope for each top-level name exported by
    // the target file. These participate in the use-def map like normal
    // definitions (shadowing, ordering), but goto-definition chases
    // through them via `resolve_definition` to reach the actual origin.
    //
    // The `local` argument is inspected only to bail: if it's set to
    // something other than TRUE/FALSE (e.g., an environment), the call
    // isn't statically analyzable and we skip it.
    //
    // TODO: In nested scopes, `local = FALSE` technically targets the
    // global environment. We currently inject into the calling scope
    // regardless to keep the sourcing mechanism simple. A future diagnostic
    // should suggest `local = TRUE` in nested contexts.
    fn collect_source_call(&mut self, call: &aether_syntax::RCall) {
        let range = call.syntax().text_trimmed_range();
        let call_offset = range.start();

        // Read back what the scan cached: the sourced files, each with its
        // resolution. The scan is the single point that extracts the paths and
        // consults `resolve_source`, so the walk never re-parses or re-resolves.
        let sourced = match self.scan.call_resolutions.get(&range) {
            Some(resolution) => resolution.source.clone(),
            None => return,
        };

        for SourcedFile { path, resolution } in sourced {
            // Record every sourced file, independent of whether it resolved.
            // `resolved` pins the canonical URL when resolution succeeded so
            // reflective queries (diagnostics for unresolved `source()`,
            // file-dependency views) read the outcome without re-resolving.
            let resolved = resolution.as_ref().map(|r| r.url.clone());
            self.walk.semantic_calls.push(SemanticCall {
                kind: SemanticCallKind::Source { path, resolved },
                offset: call_offset,
                scope: self.current_scope,
            });

            let Some(resolution) = resolution else {
                continue;
            };

            let file = resolution.url;

            for name in resolution.names {
                // Empty range: R's `source()` imports names implicitly (unlike
                // Python's `from x import y` where `y` appears in the text).
                // There's no text span to assign to these definitions.
                let name_range = TextRange::empty(call_offset);

                self.add_definition(
                    &name,
                    SymbolFlags::IS_BOUND,
                    DefinitionKind::Import {
                        call: AstPtr::new(call),
                        file: file.clone(),
                        name: name.clone(),
                    },
                    name_range,
                );
            }

            // `library()` calls inside the sourced file attach packages to R's
            // global search path at runtime, the same as a `library()` written
            // here directly would. Emit them as `Attach` semantic calls scoped
            // to this `source()`'s offset so scope-layer composition treats
            // them identically to local `library()` calls.
            for pkg in resolution.packages {
                self.walk.semantic_calls.push(SemanticCall {
                    kind: SemanticCallKind::Attach { package: pkg },
                    offset: call_offset,
                    scope: self.current_scope,
                });
            }
        }
    }

    // ## `assign()` binding
    //
    // `assign("x", value)` binds `x` in the current scope, the same as `x <-
    // value` would. We record a `DefinitionKind::Assign` def so it feeds the
    // use-def map, `exports()`, and goto exactly like a syntactic assignment.
    // The name is not chased to its value, so an `assign("f", local)` def
    // carries no NSE, just like `f <- local`.
    fn collect_assign_call(&mut self, call: &aether_syntax::RCall) {
        let range = call.syntax().text_trimmed_range();

        // Read back the bindings the scan extracted (their presence is what the
        // caller checked before dispatching here).
        let bindings = match self.scan.call_resolutions.get(&range) {
            Some(resolution) => resolution.assign.clone(),
            None => return,
        };

        self.add_assign_definitions(&AnyRExpression::RCall(call.clone()), bindings);
    }

    fn add_assign_definitions(&mut self, node: &AnyRExpression, bindings: Vec<AssignBinding>) {
        for binding in bindings {
            // The def's own range is the name token, captured at scan time, so a
            // cursor on the name at the definition site hit-tests to it, the same
            // as a syntactic `<-` binding.
            let name_range = binding.name_expr.text_trimmed_range();
            let name = binding.name_expr.as_ptr().clone();
            self.add_definition(
                &binding.name,
                SymbolFlags::IS_BOUND,
                DefinitionKind::Assign {
                    node: AstPtr::new(node),
                    name,
                    value: binding.value_expr,
                },
                name_range,
            );
        }
    }

    /// Emit the `Assign` definition for a binding operator (e.g. `x %<>% f()`) the
    /// scan recognized, after its operands were collected as uses.
    fn collect_assign_operator(&mut self, bin: &RBinaryExpression) {
        let range = bin.syntax().text_trimmed_range();
        let bindings = match self.scan.call_resolutions.get(&range) {
            Some(resolution) if !resolution.assign.is_empty() => resolution.assign.clone(),
            _ => return,
        };

        self.add_assign_definitions(&AnyRExpression::RBinaryExpression(bin.clone()), bindings);
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
                ResolvedArgumentEffect::EvalQ { env, timing } => {
                    self.collect_nse_argument(*env, *timing, &value)
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
    fn collect_nse_argument(&mut self, env: EvalEnv, timing: EvalTiming, value: &AnyRExpression) {
        match (env, timing) {
            // Calls like `evalq()`
            (EvalEnv::Current, EvalTiming::Eager) => {
                self.collect_expression(value);
            },

            // Calls like `local()`
            (EvalEnv::Nested, EvalTiming::Eager) => {
                let range = value.syntax().text_trimmed_range();
                let kind = ScopeKind::Nse(EvalEnv::Nested, EvalTiming::Eager);
                let scope = self.push_scope(kind, range);

                // Install the pending names the descent recorded for this body,
                // before collecting so lazy children inside can see them via
                // `scope_binds_anywhere()`.
                match self.scan.eager_descent.pending.remove(&range) {
                    Some(bound) => self.scan.bound_names[scope] = bound,
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

            (env, timing) => {
                let kind = ScopeKind::Nse(env, timing);
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
