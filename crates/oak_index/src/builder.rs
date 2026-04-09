use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
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

use crate::arena::Idx;
use crate::arena::IndexVec;
use crate::semantic_index::Definition;
use crate::semantic_index::DefinitionId;
use crate::semantic_index::DefinitionKind;
use crate::semantic_index::Scope;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SemanticIndex;
use crate::semantic_index::SymbolFlags;
use crate::semantic_index::SymbolTableBuilder;
use crate::semantic_index::Use;
use crate::semantic_index::UseId;

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
    current_scope: ScopeId,
}

impl SemanticIndexBuilder {
    fn new(range: TextRange) -> Self {
        let mut scopes = IndexVec::new();
        let mut symbol_tables = IndexVec::new();
        let mut definitions = IndexVec::new();
        let mut uses = IndexVec::new();

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

        Self {
            scopes,
            symbol_tables,
            definitions,
            uses,
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
        self.add_definition_in_scope(self.current_scope, name, flags, kind, range);
    }

    fn add_definition_in_scope(
        &mut self,
        scope: ScopeId,
        name: &str,
        flags: SymbolFlags,
        kind: DefinitionKind,
        range: TextRange,
    ) {
        let symbol = self.symbol_tables[scope].intern(name, flags);
        self.definitions[scope].push(Definition {
            symbol,
            kind,
            range,
        });
    }

    /// Walk from `current_scope` up through ancestors looking for a scope
    /// that already has a binding for `name`. Returns the file scope if
    /// no existing binding is found (matching R's runtime `<<-` semantics).
    fn resolve_super_target(&self, name: &str) -> ScopeId {
        let file = ScopeId::from(0);
        let mut scope = self.scopes[self.current_scope].parent;
        while let Some(id) = scope {
            if let Some(sym) = self.symbol_tables[id].get(name) {
                if sym.flags().contains(SymbolFlags::IS_BOUND) {
                    return id;
                }
            }
            scope = self.scopes[id].parent;
        }
        file
    }

    fn add_use(&mut self, name: &str, range: TextRange) {
        let symbol = self.symbol_tables[self.current_scope].intern(name, SymbolFlags::IS_USED);
        self.uses[self.current_scope].push(Use { symbol, range });
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
                let name = ident.syntax().text_trimmed().to_string();
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
                if let Ok(variable) = stmt.variable() {
                    self.add_definition(
                        &variable.syntax().text_trimmed().to_string(),
                        SymbolFlags::IS_BOUND,
                        DefinitionKind::ForVariable(stmt.syntax().clone()),
                        variable.syntax().text_trimmed_range(),
                    );
                }
                if let Ok(sequence) = stmt.sequence() {
                    self.collect_expression(&sequence);
                }
                if let Ok(body) = stmt.body() {
                    self.collect_expression(&body);
                }
            },

            AnyRExpression::RBogusExpression(_) => {},

            // Generic fallback: walk over descendant nodes and collect their
            // `AnyRExpression` children, letting `collect_expression`
            // handle their contents. This covers `RIfStatement`,
            // `RWhileStatement`, `RRepeatStatement`, `RUnaryExpression`,
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
                        &ident.syntax().text_trimmed().to_string(),
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
        match target {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.syntax().text_trimmed().to_string();
                let range = ident.syntax().text_trimmed_range();

                if super_assign {
                    let target_scope = self.resolve_super_target(&name);
                    self.add_definition_in_scope(
                        target_scope,
                        &name,
                        SymbolFlags::IS_BOUND,
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
            },

            // Complex target (`x$foo <- rhs`, `x[1] <- rhs`, etc.) does
            // not represent a binding. We recurse for uses.
            other => self.collect_expression(&other),
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

    fn finish(mut self) -> SemanticIndex {
        self.scopes[ScopeId::from(0)].descendants.end = self.scopes.next_id();
        let symbol_tables = self.symbol_tables.into_iter().map(|b| b.build()).collect();
        SemanticIndex::new(self.scopes, symbol_tables, self.definitions, self.uses)
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

fn is_super_assignment(bin: &RBinaryExpression) -> bool {
    let Ok(op) = bin.operator() else {
        return false;
    };
    matches!(
        op.kind(),
        RSyntaxKind::SUPER_ASSIGN | RSyntaxKind::SUPER_ASSIGN_RIGHT
    )
}
