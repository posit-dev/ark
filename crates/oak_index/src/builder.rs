use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::AnyRValue;
use aether_syntax::RArgumentList;
use aether_syntax::RBinaryExpression;
use aether_syntax::RExpressionList;
use aether_syntax::RFunctionDefinition;
use aether_syntax::RParameter;
use aether_syntax::RParameters;
use aether_syntax::RRoot;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstSeparatedList;
use biome_rowan::SyntaxNodeCast;
use biome_rowan::TextRange;
use biome_rowan::WalkEvent;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use oak_index_vec::Idx;
use oak_index_vec::IndexVec;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use url::Url;

use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionId;
use crate::semantic_index::DefinitionKind;
use crate::semantic_index::Directive;
use crate::semantic_index::DirectiveKind;
use crate::semantic_index::EnclosingSnapshotId;
use crate::semantic_index::EnclosingSnapshotKey;
use crate::semantic_index::Scope;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticIndex;
use crate::semantic_index::SymbolFlags;
use crate::semantic_index::SymbolTableBuilder;
use crate::semantic_index::Use;
use crate::semantic_index::UseId;
use crate::use_def_map::UseDefMapBuilder;

// TODO(salsa): Remove `semantic_index()` and variant, these are too coarse queries.

/// Build a [`SemanticIndex`] from a parsed R file.
pub fn semantic_index(root: &RRoot, file: &Url) -> SemanticIndex {
    let range = root.syntax().text_trimmed_range();
    let mut builder = SemanticIndexBuilder::new(range, file.clone(), None);
    builder.pre_scan_scope(root.syntax());
    builder.collect_expression_list(&root.expressions());
    builder.finish()
}

/// Build a [`SemanticIndex`] with cross-file `source()` resolution.
///
/// The resolver callback is called when the builder encounters a
/// `source("path")` call. It should return the sourced file's exported
/// names and any `library()` package attachments.
pub fn semantic_index_with_source_resolver<'a>(
    root: &RRoot,
    file: &Url,
    resolver: impl FnMut(&str) -> Option<SourceResolution> + 'a,
) -> SemanticIndex {
    let range = root.syntax().text_trimmed_range();
    let mut builder = SemanticIndexBuilder::new(range, file.clone(), Some(Box::new(resolver)));
    builder.pre_scan_scope(root.syntax());
    builder.collect_expression_list(&root.expressions());
    builder.finish()
}

/// The result of resolving a `source()` call. Returned by the resolver
/// callback passed to the builder.
pub struct SourceResolution {
    /// The resolved URL of the sourced file.
    pub file: Url,

    /// Names of top-level definitions in the sourced file.
    pub names: Vec<String>,

    /// Package names from `library()` directives in the sourced file
    /// (and transitively from files it sources).
    pub packages: Vec<String>,
}

type SourceResolver<'a> = Box<dyn FnMut(&str) -> Option<SourceResolution> + 'a>;

// Maintains the preorder allocation invariant on `Scope::descendants`. The
// parallel arrays are pushed in lockstep so they stay indexed by the same
// `ScopeId`.
struct SemanticIndexBuilder<'a> {
    scopes: IndexVec<ScopeId, Scope>,
    symbol_tables: IndexVec<ScopeId, SymbolTableBuilder>,
    definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
    uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
    use_def_maps: IndexVec<ScopeId, UseDefMapBuilder>,
    current_scope: ScopeId,
    pre_scans: IndexVec<ScopeId, PreScanScope>,
    enclosing_snapshots: FxHashMap<EnclosingSnapshotKey, (ScopeId, EnclosingSnapshotId)>,
    directives: Vec<Directive>,
    file: Url,
    source_resolver: Option<SourceResolver<'a>>,
}

impl<'a> SemanticIndexBuilder<'a> {
    fn new(range: TextRange, file: Url, source_resolver: Option<SourceResolver<'a>>) -> Self {
        let mut scopes = IndexVec::new();
        let mut symbol_tables = IndexVec::new();
        let mut definitions = IndexVec::new();
        let mut uses = IndexVec::new();
        let mut use_def_maps = IndexVec::new();
        let mut pre_scans = IndexVec::new();

        // The descendants range starts empty (`n+1..n+1`). `pop_scope` later
        // fills in `descendants.end` with the current arena length. Everything
        // allocated between push and pop is a descendant.
        let file_scope = scopes.push(Scope {
            parent: None,
            kind: ScopeKind::File,
            range,
            descendants: ScopeId::from(1)..ScopeId::from(1),
        });

        // All `ScopeId`-indexed vecs must be pushed in lockstep so they stay
        // the same length. The `push_scope()` method is in charge of
        // guaranteeing that invariant after construction.
        symbol_tables.push(SymbolTableBuilder::new());
        definitions.push(IndexVec::new());
        uses.push(IndexVec::new());
        use_def_maps.push(UseDefMapBuilder::new());
        pre_scans.push(PreScanScope::new());

        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
            use_def_maps,
            current_scope: file_scope,
            pre_scans,
            enclosing_snapshots: FxHashMap::default(),
            directives: Vec::new(),
            file,
            source_resolver,
        }
    }

    fn push_scope(&mut self, kind: ScopeKind, range: TextRange) -> ScopeId {
        let parent = Some(self.current_scope);
        let next_raw = self.scopes.next_id().index() as u32;

        // Descendants start right after this scope. `end` is later filled in by
        // `pop_scope`.
        let descendants = ScopeId::from(next_raw + 1)..ScopeId::from(next_raw + 1);

        let id = self.scopes.push(Scope {
            parent,
            kind,
            range,
            descendants,
        });
        self.current_scope = id;

        self.symbol_tables.push(SymbolTableBuilder::new());
        self.definitions.push(IndexVec::new());
        self.uses.push(IndexVec::new());
        self.use_def_maps.push(UseDefMapBuilder::new());
        self.pre_scans.push(PreScanScope::new());

        id
    }

    fn pop_scope(&mut self, id: ScopeId) {
        // Close the descendants range: everything allocated from `push_scope()`
        // to here is a descendant.
        self.scopes[id].descendants.end = self.scopes.next_id();
        self.current_scope = match self.scopes[id].parent {
            Some(parent) => parent,
            None => panic!("`pop_scope()` called on the file scope"),
        };
    }

    fn add_definition(
        &mut self,
        name: &str,
        flags: SymbolFlags,
        kind: DefinitionKind,
        range: TextRange,
    ) {
        let symbol_id = self.symbol_tables[self.current_scope].intern(name, flags);
        let def_id = self.definitions[self.current_scope].push(Definition {
            symbol: symbol_id,
            kind,
            range,
            file: self.file.clone(),
            scope: self.current_scope,
        });
        self.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
        self.use_def_maps[self.current_scope].record_definition(symbol_id, def_id);
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
        let symbol_id =
            self.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_SUPER_BOUND);
        self.definitions[self.current_scope].push(Definition {
            symbol: symbol_id,
            kind: kind.clone(),
            range,
            file: self.file.clone(),
            scope: self.current_scope,
        });

        let target_scope = self.resolve_super_target(name);

        let target_symbol = self.symbol_tables[target_scope].intern(name, SymbolFlags::IS_BOUND);
        let target_def_id = self.definitions[target_scope].push(Definition {
            symbol: target_symbol,
            kind,
            range,
            file: self.file.clone(),
            scope: target_scope,
        });
        self.use_def_maps[target_scope].ensure_symbol(target_symbol);
        self.use_def_maps[target_scope].record_deferred_definition(target_symbol, target_def_id);
    }

    // Walk up from the parent scope looking for a scope where `name` already
    // has `IS_BOUND`. Returns that scope, or the file scope if no binding is
    // found (mirroring R's assignment to the global environment).
    fn resolve_super_target(&self, name: &str) -> ScopeId {
        let file_scope = ScopeId::from(0);
        let Some(mut scope) = self.scopes[self.current_scope].parent else {
            return file_scope;
        };

        loop {
            if let Some(id) = self.symbol_tables[scope].id(name) {
                if self.symbol_tables[scope]
                    .symbol(id)
                    .flags()
                    .contains(SymbolFlags::IS_BOUND)
                {
                    return scope;
                }
            }
            let Some(parent) = self.scopes[scope].parent else {
                return file_scope;
            };
            scope = parent;
        }
    }

    fn add_use(&mut self, name: &str, range: TextRange) {
        let symbol_id = self.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_USED);
        let use_id = self.uses[self.current_scope].push(Use {
            symbol: symbol_id,
            range,
        });
        self.use_def_maps[self.current_scope].ensure_symbol(symbol_id);
        self.use_def_maps[self.current_scope].record_use(symbol_id, use_id);

        // Associate free variables with the enclosing snapshot where the
        // variable is defined
        if self.use_def_maps[self.current_scope].is_may_be_unbound(symbol_id) {
            let use_key = EnclosingSnapshotKey {
                nested_scope: self.current_scope,
                nested_symbol: symbol_id,
            };
            self.register_enclosing_snapshot(name, use_key);
        }
    }

    fn register_enclosing_snapshot(&mut self, name: &str, use_key: EnclosingSnapshotKey) {
        // We're looking for a parent definition for this scope's free variable
        // so start from parent
        let Some(mut current_scope) = self.scopes[self.current_scope].parent else {
            return;
        };

        loop {
            let found_by_flag = self.symbol_tables[current_scope]
                .id(name)
                .is_some_and(|sym_id| {
                    self.symbol_tables[current_scope]
                        .symbol(sym_id)
                        .flags()
                        .contains(SymbolFlags::IS_BOUND)
                });

            let found_by_prescan = self.pre_scans[current_scope].has_name(name);

            if found_by_flag || found_by_prescan {
                // Intern with empty flags: we just need a stable `SymbolId` for
                // the lookup key. If found via `found_by_flag`, the symbol
                // already exists with `IS_BOUND`. If found via pre-scan only,
                // the later `add_definition` call during the full walk will set
                // `IS_BOUND`.
                let enclosing_symbol_id =
                    self.symbol_tables[current_scope].intern(name, SymbolFlags::empty());

                if self.enclosing_snapshots.contains_key(&use_key) {
                    return;
                }

                self.use_def_maps[current_scope].ensure_symbol(enclosing_symbol_id);
                let snapshot_id = self.use_def_maps[current_scope]
                    .register_enclosing_snapshot(enclosing_symbol_id);
                self.enclosing_snapshots
                    .insert(use_key, (current_scope, snapshot_id));

                return;
            }

            let Some(parent) = self.scopes[current_scope].parent else {
                return;
            };
            current_scope = parent;
        }
    }

    // --- Recursive descent ---

    fn collect_expression_list(&mut self, list: &RExpressionList) {
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
                    if let Ok(lhs) = bin.left() {
                        self.collect_expression(&lhs);
                    }
                    if let Ok(rhs) = bin.right() {
                        self.collect_expression(&rhs);
                    }
                }
            },

            // Calls and subsets need explicit handling because argument name
            // clauses contain `RIdentifier` nodes that should not be recorded
            // as uses.
            AnyRExpression::RCall(call) => {
                if let Ok(func) = call.function() {
                    self.collect_expression(&func);
                }
                if let Ok(args) = call.arguments() {
                    self.collect_arguments(&args.items());
                }
                // TODO(nse): When eager NSE scopes land (e.g. `local()`) we should
                // also consider nested scopes as long as they're not lazy (e.g.
                // function definitions or NSE calls that don't evaluate
                // immediately.
                self.collect_directive(call);
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

            AnyRExpression::RNamespaceExpression(_) => {
                // In `pkg::fn` or `pkg:::fn`, both sides are selectors, not
                // variable references in the current scope
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
                        DefinitionKind::ForVariable(stmt.syntax().clone()),
                        variable.syntax().text_trimmed_range(),
                    );
                }
                if let Ok(sequence) = stmt.sequence() {
                    self.collect_expression(&sequence);
                }

                let pre_loop = self.use_def_maps[self.current_scope].snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.uses[self.current_scope],
                    );
                }

                self.use_def_maps[self.current_scope].merge(pre_loop);
            },

            AnyRExpression::RIfStatement(stmt) => {
                // Condition is always evaluated
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_if = self.use_def_maps[self.current_scope].snapshot();

                // If-body (consequence)
                if let Ok(consequence) = stmt.consequence() {
                    self.collect_expression(&consequence);
                }

                let post_if = self.use_def_maps[self.current_scope].snapshot();
                self.use_def_maps[self.current_scope].restore(pre_if);

                // Else-body (alternative), if present. If absent, the
                // "else path" is just the pre-if state we restored to.
                if let Some(else_clause) = stmt.else_clause() {
                    if let Ok(alternative) = else_clause.alternative() {
                        self.collect_expression(&alternative);
                    }
                }

                // After: definitions from both branches are live
                self.use_def_maps[self.current_scope].merge(post_if);
            },

            AnyRExpression::RWhileStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_loop = self.use_def_maps[self.current_scope].snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.uses[self.current_scope],
                    );
                }

                // Body may not execute
                self.use_def_maps[self.current_scope].merge(pre_loop);
            },

            AnyRExpression::RRepeatStatement(stmt) => {
                // Body always executes at least once, so no merge with pre-loop state.
                if let Ok(body) = stmt.body() {
                    let pre_loop = self.use_def_maps[self.current_scope].snapshot();
                    let first_use = self.uses[self.current_scope].next_id();
                    self.collect_expression(&body);
                    self.use_def_maps[self.current_scope].finish_loop_defs(
                        &pre_loop,
                        first_use,
                        &self.uses[self.current_scope],
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
            self.collect_parameters(&params);
        }
        if let Ok(body) = fun.body() {
            self.pre_scan_scope(body.syntax());
            self.collect_expression(&body);
        }

        self.pop_scope(scope);
    }

    /// Pre-scan a scope to collect all definition names (skipping nested
    /// function bodies). Runs before the full walk so that enclosing
    /// snapshot registration can find where free variables are bound,
    /// even when the walk in the parent scope hasn't reached the
    /// definition yet. Must stay in sync with the full walk's definition
    /// handling: any construct that calls `add_definition` should have a
    /// corresponding entry here.
    fn pre_scan_scope(&mut self, node: &RSyntaxNode) {
        let mut preorder = node.preorder();
        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };
            let Some(expr) = AnyRExpression::cast(node) else {
                continue;
            };
            match &expr {
                // NSE scopes (e.g. `local({...})`) will also need to
                // be skipped here once recognized, since their
                // definitions belong to a child scope.
                AnyRExpression::RFunctionDefinition(_) => {
                    preorder.skip_subtree();
                },
                AnyRExpression::RBinaryExpression(bin) if is_assignment(bin) => {
                    if !is_super_assignment(bin) {
                        let right = is_right_assignment(bin);
                        let target = if right { bin.right() } else { bin.left() };
                        if let Ok(target) = target {
                            if let Some((name, range)) = assignment_name(&target) {
                                self.pre_scans[self.current_scope].add(name, range);
                            }
                        }
                    }
                },
                AnyRExpression::RForStatement(stmt) => {
                    if let Ok(variable) = stmt.variable() {
                        self.pre_scans[self.current_scope]
                            .add(variable.name_text(), variable.syntax().text_trimmed_range());
                    }
                },
                _ => {},
            }
        }
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
                        DefinitionKind::Parameter(param.syntax().clone()),
                        ident.syntax().text_trimmed_range(),
                    );
                },
                AnyRParameterName::RDots(dots) => {
                    self.add_definition(
                        "...",
                        flags,
                        DefinitionKind::Parameter(param.syntax().clone()),
                        dots.syntax().text_trimmed_range(),
                    );
                },
                AnyRParameterName::RDotDotI(ddi) => {
                    self.add_definition(
                        &ddi.syntax().text_trimmed().to_string(),
                        flags,
                        DefinitionKind::Parameter(param.syntax().clone()),
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
                DefinitionKind::SuperAssignment(op.syntax().clone()),
                range,
            );
        } else {
            self.add_definition(
                &name,
                SymbolFlags::IS_BOUND,
                DefinitionKind::Assignment(op.syntax().clone()),
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

    fn collect_directive(&mut self, call: &aether_syntax::RCall) {
        let Ok(AnyRExpression::RIdentifier(ident)) = call.function() else {
            return;
        };

        let fn_name = ident.name_text();
        if fn_name == "library" || fn_name == "require" {
            self.collect_attach_directive(call);
        } else if fn_name == "source" {
            self.collect_source_directive(call);
        }
    }

    // ## `library()` / `require()` scoping
    //
    // In R, `library()` always modifies the global search path regardless
    // of where it's called. Statically, we scope the directive to
    // `self.current_scope`: at file scope it's visible everywhere (sequential
    // execution is guaranteed), but inside a function it's only visible
    // within that function and its children, since the function might never
    // be called. Same reasoning as `source()` directives.
    fn collect_attach_directive(&mut self, call: &aether_syntax::RCall) {
        let Ok(args) = call.arguments() else {
            return;
        };
        let mut items = args.items().iter();

        // For now, only recognise exactly one unnamed argument. We'll do
        // argument matching later (`character.only` unquoting is another
        // complication).
        let Some(Ok(first_arg)) = items.next() else {
            return;
        };
        if first_arg.name_clause().is_some() || items.next().is_some() {
            return;
        }
        let Some(value) = first_arg.value() else {
            return;
        };

        let pkg_name = match &value {
            AnyRExpression::RIdentifier(ident) => Some(ident.name_text()),
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
            _ => None,
        };
        let Some(pkg_name) = pkg_name else {
            return;
        };

        let call_offset = call.syntax().text_trimmed_range().start();
        self.directives.push(Directive {
            kind: DirectiveKind::Attach(pkg_name),
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
    fn collect_source_directive(&mut self, call: &aether_syntax::RCall) {
        let Ok(args) = call.arguments() else {
            return;
        };

        let mut path: Option<String> = None;
        let mut bail = false;

        for item in args.items().iter() {
            let Ok(arg) = item else { continue };

            if let Some(name_clause) = arg.name_clause() {
                let Ok(AnyRArgumentName::RIdentifier(name_ident)) = name_clause.name() else {
                    continue;
                };
                if name_ident.name_text() == "local" {
                    if let Some(value) = arg.value() {
                        match value {
                            // TRUE/FALSE are fine, we resolve uniformly. For
                            // the FALSE in nested context case, we'll emit a
                            // diagnostic.
                            AnyRExpression::RTrueExpression(_) |
                            AnyRExpression::RFalseExpression(_) => {},
                            // With anything else (environment, non-statically
                            // resolvable expression) is not we need to bail.
                            _ => bail = true,
                        }
                    }
                }
            } else if path.is_none() {
                // First positional argument: the file path
                if let Some(AnyRExpression::AnyRValue(AnyRValue::RStringValue(s))) = arg.value() {
                    path = s.string_text();
                }
            }
        }

        if bail {
            return;
        }

        let Some(path) = path else {
            return;
        };

        let call_offset = call.syntax().text_trimmed_range().start();

        let Some(resolution) = self.resolve_source(&path) else {
            return;
        };

        let file = resolution.file;

        for name in resolution.names {
            // Empty range: R's `source()` imports names implicitly (unlike
            // Python's `from x import y` where `y` appears in the text).
            // There's no text span to assign to these definitions.
            let range = TextRange::empty(call_offset);

            self.add_definition(
                &name,
                SymbolFlags::IS_BOUND,
                DefinitionKind::Import {
                    call: call.syntax().clone(),
                    file: file.clone(),
                    name: name.clone(),
                },
                range,
            );
        }

        for pkg in resolution.packages {
            self.directives.push(Directive {
                kind: DirectiveKind::Attach(pkg),
                offset: call_offset,
                scope: self.current_scope,
            });
        }
    }

    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        let source_resolver = self.source_resolver.as_mut()?;
        source_resolver(path)
    }

    fn finish(mut self) -> SemanticIndex {
        self.scopes[ScopeId::from(0)].descendants.end = self.scopes.next_id();

        let symbol_tables = self.symbol_tables.into_iter().map(|b| b.build()).collect();
        let use_def_maps: IndexVec<ScopeId, _> = self
            .use_def_maps
            .into_iter()
            .zip(self.uses.iter())
            .map(|(b, (_, uses))| b.finish(uses))
            .collect();

        SemanticIndex::new(
            self.scopes,
            symbol_tables,
            self.definitions,
            self.uses,
            use_def_maps,
            self.enclosing_snapshots,
            self.directives,
        )
    }
}

/// All definitions in a scope, collected before the full walk. Skips nested
/// function bodies (those belong to child scopes). Two consumers:
///
/// - Enclosing snapshots: `has_name()` checks whether a symbol will be
///   defined in an ancestor scope (even when the ancestor's walk hasn't reached
///   that definition yet), so that `register_enclosing_snapshot()` can find the
///   right ancestor for free variables.
/// - NSE resolution: With NSE, each function call potentially pushes a scope
///   (which can be lazy or eager). We need to resolve the called function's
///   semantic during the walk. Inside lazy scopes (e.g. function bodies),
///   `by_name` provides the complete set of parent definitions so that the
///   function can be resolved against all the parent scope's definitions (if NSE
///   semantics don't match across definitions, we pick one and lint). Intra-scope
///   resolution is linear and uses the current `symbol_states` directly instead.
struct PreScanScope {
    _defs: Vec<PreScanDef>,
    by_name: FxHashMap<String, SmallVec<[usize; 2]>>,
}

/// A single definition site found during the pre-scan. Fields are not
/// read yet but will be used for NSE lookup.
struct PreScanDef {
    _name: String,
    _range: TextRange,
}

impl PreScanScope {
    fn new() -> Self {
        Self {
            _defs: Vec::new(),
            by_name: FxHashMap::default(),
        }
    }

    fn add(&mut self, name: String, range: TextRange) {
        let idx = self._defs.len();
        self.by_name.entry(name.clone()).or_default().push(idx);
        self._defs.push(PreScanDef {
            _name: name,
            _range: range,
        });
    }

    fn has_name(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }
}

fn is_assignment(bin: &RBinaryExpression) -> bool {
    let Ok(op) = bin.operator() else {
        return false;
    };
    matches!(
        op.kind(),
        RSyntaxKind::ASSIGN |
            RSyntaxKind::EQUAL |
            RSyntaxKind::ASSIGN_RIGHT |
            RSyntaxKind::SUPER_ASSIGN |
            RSyntaxKind::SUPER_ASSIGN_RIGHT
    )
}

fn is_right_assignment(bin: &RBinaryExpression) -> bool {
    let Ok(op) = bin.operator() else {
        return false;
    };
    matches!(
        op.kind(),
        RSyntaxKind::ASSIGN_RIGHT | RSyntaxKind::SUPER_ASSIGN_RIGHT
    )
}

/// Extract the binding name and range from an assignment target expression.
/// Returns `None` for complex targets (`x$foo`, `x[1]`, etc.) that don't
/// represent simple name bindings.
fn assignment_name(target: &AnyRExpression) -> Option<(String, TextRange)> {
    match target {
        AnyRExpression::RIdentifier(ident) => {
            let name = ident.name_text();
            let range = ident.syntax().text_trimmed_range();
            Some((name, range))
        },
        // `"x" <- 1` is equivalent to `x <- 1` in R
        AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            let name = s.string_text()?;
            let range = s.syntax().text_trimmed_range();
            Some((name, range))
        },
        _ => None,
    }
}

fn is_super_assignment(bin: &RBinaryExpression) -> bool {
    let Ok(op) = bin.operator() else {
        return false;
    };
    matches!(
        op.kind(),
        RSyntaxKind::SUPER_ASSIGN | RSyntaxKind::SUPER_ASSIGN_RIGHT
    )
}
