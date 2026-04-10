use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::AnyRValue;
use aether_syntax::RArgumentList;
use aether_syntax::RBinaryExpression;
use aether_syntax::RExpressionList;
use aether_syntax::RForStatement;
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

use crate::index_vec::Idx;
use crate::index_vec::IndexVec;
use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionId;
use crate::semantic_index::DefinitionKind;
use crate::semantic_index::Scope;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticIndex;
use crate::semantic_index::SymbolFlags;
use crate::semantic_index::SymbolId;
use crate::semantic_index::SymbolTableBuilder;
use crate::semantic_index::Use;
use crate::semantic_index::UseId;
use crate::use_def_map::UseDefMap;
use crate::use_def_map::UseDefMapBuilder;

/// Build a [`SemanticIndex`] from a parsed R file.
pub fn build(root: &RRoot) -> SemanticIndex {
    let range = root.syntax().text_trimmed_range();
    let mut builder = SemanticIndexBuilder::new(range);
    builder.collect_expression_list(&root.expressions());
    builder.finish()
}

// Maintains the preorder allocation invariant on `Scope::descendants`. The
// parallel arrays are pushed in lockstep so they stay indexed by the same
// `ScopeId`.
struct SemanticIndexBuilder {
    scopes: IndexVec<ScopeId, Scope>,
    symbol_tables: IndexVec<ScopeId, SymbolTableBuilder>,
    definitions: IndexVec<ScopeId, IndexVec<DefinitionId, Definition>>,
    uses: IndexVec<ScopeId, IndexVec<UseId, Use>>,
    use_def_maps: IndexVec<ScopeId, UseDefMap>,
    current_use_def: UseDefMapBuilder,
    use_def_stack: Vec<UseDefMapBuilder>,
    current_scope: ScopeId,
}

impl SemanticIndexBuilder {
    fn new(range: TextRange) -> Self {
        let mut scopes = IndexVec::new();
        let mut symbol_tables = IndexVec::new();
        let mut definitions = IndexVec::new();
        let mut uses = IndexVec::new();
        let mut use_def_maps = IndexVec::new();

        // The descendants range starts empty (`n+1..n+1`). `pop_scope` later
        // fills in `descendants.end` with the current arena length. Everything
        // allocated between push and pop is a descendant.
        let file = scopes.push(Scope {
            parent: None,
            kind: ScopeKind::File,
            range,
            descendants: ScopeId::from(1)..ScopeId::from(1),
        });

        symbol_tables.push(SymbolTableBuilder::new());
        definitions.push(IndexVec::new());
        uses.push(IndexVec::new());
        use_def_maps.push(UseDefMap::empty());

        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
            use_def_maps,
            current_use_def: UseDefMapBuilder::new(),
            use_def_stack: Vec::new(),
            current_scope: file,
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
        self.use_def_maps.push(UseDefMap::empty());

        let parent_use_def = std::mem::replace(&mut self.current_use_def, UseDefMapBuilder::new());
        self.use_def_stack.push(parent_use_def);

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

        let parent_use_def = match self.use_def_stack.pop() {
            Some(builder) => builder,
            None => panic!("`pop_scope()` called with empty use-def stack"),
        };
        let finalized = std::mem::replace(&mut self.current_use_def, parent_use_def).finish();
        self.use_def_maps[id] = finalized;
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
        });

        self.current_use_def.ensure_symbol(symbol_id);
        self.current_use_def.record_binding(symbol_id, def_id);
    }

    // Super-assignment is lexically in the current scope but binds in an
    // ancestor. We record the definition here (for go-to-definition, rename)
    // but skip use-def tracking since the binding doesn't affect local flow.
    fn add_super_definition(&mut self, name: &str, kind: DefinitionKind, range: TextRange) {
        let symbol_id =
            self.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_SUPER_BOUND);
        self.definitions[self.current_scope].push(Definition {
            symbol: symbol_id,
            kind,
            range,
        });
    }

    fn add_use(&mut self, name: &str, range: TextRange) {
        let symbol_id = self.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_USED);
        let use_id = self.uses[self.current_scope].push(Use {
            symbol: symbol_id,
            range,
        });

        self.current_use_def.ensure_symbol(symbol_id);
        self.current_use_def.record_use(symbol_id, use_id);
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
                let name = identifier_text(ident);
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
                        &identifier_text(&variable),
                        SymbolFlags::IS_BOUND,
                        DefinitionKind::ForVariable(stmt.syntax().clone()),
                        variable.syntax().text_trimmed_range(),
                    );
                }
                if let Ok(sequence) = stmt.sequence() {
                    self.collect_expression(&sequence);
                }

                let pre_loop = self.current_use_def.snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.uses[self.current_scope].next_id();
                    let loop_header = self.build_loop_header(body.syntax());
                    self.collect_expression(&body);
                    self.finish_loop_header(&loop_header, first_use);
                }

                self.current_use_def.merge(pre_loop);
            },

            AnyRExpression::RIfStatement(stmt) => {
                // Condition is always evaluated
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_if = self.current_use_def.snapshot();

                // If-body (consequence)
                if let Ok(consequence) = stmt.consequence() {
                    self.collect_expression(&consequence);
                }

                let post_if = self.current_use_def.snapshot();
                self.current_use_def.restore(pre_if);

                // Else-body (alternative), if present. If absent, the
                // "else path" is just the pre-if state we restored to.
                if let Some(else_clause) = stmt.else_clause() {
                    if let Ok(alternative) = else_clause.alternative() {
                        self.collect_expression(&alternative);
                    }
                }

                // After: definitions from both branches are live
                self.current_use_def.merge(post_if);
            },

            AnyRExpression::RWhileStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.collect_expression(&condition);
                }

                let pre_loop = self.current_use_def.snapshot();

                if let Ok(body) = stmt.body() {
                    let first_use = self.uses[self.current_scope].next_id();
                    let loop_header = self.build_loop_header(body.syntax());
                    self.collect_expression(&body);
                    self.finish_loop_header(&loop_header, first_use);
                }

                // Body may not execute
                self.current_use_def.merge(pre_loop);
            },

            AnyRExpression::RRepeatStatement(stmt) => {
                // Body always executes at least once, no snapshot needed
                if let Ok(body) = stmt.body() {
                    let first_use = self.uses[self.current_scope].next_id();
                    let loop_header = self.build_loop_header(body.syntax());
                    self.collect_expression(&body);
                    self.finish_loop_header(&loop_header, first_use);
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
                        &identifier_text(ident),
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

        let Some((name, range)) = assignment_target_name(&target) else {
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

    // Pre-walk a loop body to find all symbols that will be bound, then
    // create `LoopHeader` placeholder definitions for each. These are
    // recorded as additional (non-shadowing) bindings so that uses at the
    // top of the body can see definitions from a previous iteration.
    // After the body is visited, `finish_loop_header` resolves each
    // placeholder to the real definitions that are live at the end of
    // the body.
    //
    // Skips function bodies (different scope) and super-assignments (don't
    // affect local flow).
    fn build_loop_header(&mut self, body: &RSyntaxNode) -> Vec<(SymbolId, DefinitionId)> {
        let names = Self::collect_loop_bound_names(body);
        let mut loop_header = Vec::new();

        for name in names {
            let symbol_id =
                self.symbol_tables[self.current_scope].intern(&name, SymbolFlags::IS_BOUND);
            let def_id = self.definitions[self.current_scope].push(Definition {
                symbol: symbol_id,
                kind: DefinitionKind::LoopHeader,
                range: body.text_trimmed_range(),
            });

            self.current_use_def.ensure_symbol(symbol_id);
            self.current_use_def.record_loop_binding(symbol_id, def_id);
            loop_header.push((symbol_id, def_id));
        }

        loop_header
    }

    fn finish_loop_header(&mut self, loop_header: &[(SymbolId, DefinitionId)], first_use: UseId) {
        for &(symbol_id, placeholder_id) in loop_header {
            self.current_use_def
                .resolve_placeholder(symbol_id, placeholder_id, first_use);
        }
    }

    // Keep in sync with `collect_expression`: Every construct that creates
    // a definition there must be matched here so that loop headers account
    // for all bindings in the body.
    fn collect_loop_bound_names(body: &RSyntaxNode) -> Vec<String> {
        let mut names = Vec::new();
        let mut preorder = body.preorder();

        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(node) = event else {
                continue;
            };

            match node.kind() {
                // Function bodies are separate scopes. In the future we'll need
                // an indirection here to handle other kinds of local scopes, in
                // particular from NSE functions like `local()`.
                RSyntaxKind::R_FUNCTION_DEFINITION => {
                    preorder.skip_subtree();
                },

                RSyntaxKind::R_BINARY_EXPRESSION => {
                    let op: RBinaryExpression = node.cast().unwrap();
                    if let Some((name, _)) = assignment_target(&op) {
                        names.push(name);
                    }
                },

                RSyntaxKind::R_FOR_STATEMENT => {
                    let for_stmt: RForStatement = node.cast().unwrap();
                    if let Ok(variable) = for_stmt.variable() {
                        names.push(identifier_text(&variable));
                    }
                },

                _ => {},
            }
        }

        names.sort();
        names.dedup();
        names
    }

    fn collect_arguments(&mut self, args: &RArgumentList) {
        for item in args.iter() {
            let Ok(arg) = item else { continue };
            if let Some(value) = arg.value() {
                self.collect_expression(&value);
            }
        }
    }

    fn finish(mut self) -> SemanticIndex {
        self.scopes[ScopeId::from(0)].descendants.end = self.scopes.next_id();

        let file_use_def_map = self.current_use_def.finish();
        self.use_def_maps[ScopeId::from(0)] = file_use_def_map;

        let symbol_tables = self.symbol_tables.into_iter().map(|b| b.build()).collect();
        SemanticIndex::new(
            self.scopes,
            symbol_tables,
            self.definitions,
            self.uses,
            self.use_def_maps,
        )
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

/// Extract the name of an `RIdentifier`, stripping backticks if present.
///
/// Backtick-quoted identifiers like `` `my var` `` are parsed as `RIdentifier`
/// nodes whose `text_trimmed()` includes the backticks. The backticks are a
/// quoting mechanism, not part of the symbol name.
fn identifier_text(ident: &aether_syntax::RIdentifier) -> String {
    let text = ident.syntax().text_trimmed().to_string();
    match text.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
        Some(inner) => inner.to_string(),
        None => text,
    }
}

/// Extract the unquoted text of an `RStringValue`.
///
/// Note: `RStringValue::inner_string_text()` from aether_syntax would be the
/// idiomatic API for this, but it delegates to the free `inner_string_text()`
/// which checks for node kind `R_STRING_VALUE` instead of token kind
/// `R_STRING_LITERAL`, so it never actually strips the delimiters.
fn string_value_text(s: &aether_syntax::RStringValue) -> Option<String> {
    let token = s.value_token().ok()?;
    let text = token.text_trimmed();
    Some(text[1..text.len() - 1].to_string())
}

/// For a local (non-super) assignment, extract the binding name and range.
/// Returns `None` if the expression is not an assignment, is a
/// super-assignment, or has a complex target (`x$foo`, `x[1]`, etc.).
fn assignment_target(bin: &RBinaryExpression) -> Option<(String, TextRange)> {
    if !is_assignment(bin) || is_super_assignment(bin) {
        return None;
    }
    let right = is_right_assignment(bin);
    let target = if right { bin.right() } else { bin.left() }.ok()?;
    assignment_target_name(&target)
}

/// Extract the binding name and range from an assignment target expression.
/// Returns `None` for complex targets (`x$foo`, `x[1]`, etc.) that don't
/// represent simple name bindings.
fn assignment_target_name(target: &AnyRExpression) -> Option<(String, TextRange)> {
    match target {
        AnyRExpression::RIdentifier(ident) => {
            let name = identifier_text(ident);
            let range = ident.syntax().text_trimmed_range();
            Some((name, range))
        },
        // `"x" <- 1` is equivalent to `x <- 1` in R
        AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
            let name = string_value_text(s)?;
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
