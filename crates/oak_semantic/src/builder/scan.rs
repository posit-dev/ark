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
use super::is_assignment;
use super::is_right_assignment;
use super::is_super_assignment;
use super::SemanticIndexBuilder;
use crate::effects::AssignBinding;
use crate::effects::ResolvedArgumentEffect;
use crate::effects::ResolvedArgumentEffects;
use crate::effects::ScopeBindings;
use crate::resolver::ImportsResolver;
use crate::resolver::SourceResolution;
use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;
use crate::semantic_index::ScopeId;
use crate::semantic_index::ScopeKind;
use crate::semantic_index::SymbolFlags;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    /// Reset the flow-precise binding state for a fresh scope's scan.
    ///
    /// Seeds it with two things:
    ///
    /// - The names inherited from enclosing scopes, captured when this scope was
    ///   entered (`enclosing_flow`). The parent's own scan was seeded the same
    ///   way, so this is transitively complete: it holds every eager binding
    ///   visible from an ancestor at this scope's definition point.
    /// - The scope's own already-bound names. For a function scope that's the
    ///   parameters, recorded just before the scan runs. For file and NSE scopes
    ///   nothing local is bound yet.
    ///
    /// Parameter defaults are a special case: they are scanned before the params
    /// are recorded, so `walk_function` seeds the full formal set by hand
    /// (all formals bind at once in R, so a default sees every parameter name).
    pub(super) fn begin_scan(&mut self) {
        let range = self.scopes[self.current_scope].range;

        match self.scan.enclosing_flow.get(&range).cloned() {
            Some(entry) => self.scan.flow_state.restore(entry),
            None => self.scan.flow_state.clear(),
        }

        for (_id, symbol) in self.walk.symbol_tables[self.current_scope].iter() {
            if symbol.flags().contains(SymbolFlags::IS_BOUND) {
                self.scan.flow_state.bind(symbol.name().to_string());
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
    /// non-skipped definition names to `bound_names`. The bound names must be
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
    ///   `flow_state`, and the names it binds are left pending for the walk to
    ///   install without re-scanning.
    /// - Function and lazy bodies (`Nested + Lazy`, `Current + Lazy`) are their
    ///   own scan units, scanned separately when the walk enters them, because
    ///   NSE resolution there needs the child's own flow context.
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
                // shadows it (see `enclosing_flow`).
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
                // A loop body only adds bindings (a name bound inside still
                // "reaches" on the ran path), so no restore is needed, unlike
                // the two-branch `if`/`else` below.
                if let Ok(body) = stmt.body() {
                    self.scan_expression(&body);
                }
            },

            AnyRExpression::RIfStatement(stmt) => {
                if let Ok(condition) = stmt.condition() {
                    self.scan_expression(&condition);
                }

                let pre_if = self.scan.flow_state.snapshot();

                if let Ok(consequence) = stmt.consequence() {
                    self.scan_expression(&consequence);
                }

                let post_if = self.scan.flow_state.snapshot();
                self.scan.flow_state.restore(pre_if);

                if let Some(else_clause) = stmt.else_clause() {
                    if let Ok(alternative) = else_clause.alternative() {
                        self.scan_expression(&alternative);
                    }
                }

                // Both branches' bindings are live afterwards.
                self.scan.flow_state.merge(post_if);
            },

            // `while`/`repeat` loops, subsets, extractions, parentheses, unary
            // ops, and literals: recurse into child expressions. Loops need no
            // flow restore (see the `for` arm). Identifiers and dots are leaves
            // with no bindings or calls, so they fall through to a no-op walk.
            _ => {
                self.scan_descendants(expr.syntax());
            },
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
            self.scan
                .call_resolutions
                .entry(call.syntax().text_trimmed_range())
                .or_default()
                .attach = Some(package.clone());
            if !self.scopes[self.current_scope].kind.is_lazy() {
                self.scan.attached_flow.push(package);
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

                    // Calls like `on_load()`. Its body runs later, so its defs
                    // land in the enclosing scope. We don't resolve the body's
                    // calls here. The walk does that once it enters the child
                    // scope. But we do grab the names it defines now, so the
                    // owner's bound names are complete before the walk reaches a sibling.
                    (EvalEnv::Current, EvalTiming::Lazy) => {
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
                    (EvalEnv::Nested, EvalTiming::Eager) => {
                        let old = self.scan.flow_state.snapshot();

                        let range = value.syntax().text_trimmed_range();
                        self.scan.eager_descent.open.push(BoundNames::new());
                        self.scan_expression(&value);
                        if let Some(bound) = self.scan.eager_descent.open.pop() {
                            self.scan.eager_descent.pending.insert(range, bound);
                        }

                        self.scan.flow_state.restore(old);
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
    pub(super) fn scan_lazy_owner_bindings(&mut self, expr: &AnyRExpression) {
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

    /// Scan a binary operator for an assign effect (e.g. magrittr's `x %<>% f()`)
    pub(super) fn scan_operator_assign(&mut self, bin: &RBinaryExpression) {
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
        // Seed `flow_state` with every parameter names so a callee inside a
        // default value sees the full formal set
        for param in params.items().iter() {
            let Ok(param) = param else { continue };
            let Ok(name) = param.name() else { continue };
            let text = match &name {
                AnyRParameterName::RIdentifier(ident) => ident.name_text(),
                AnyRParameterName::RDots(_) => String::from("..."),
                AnyRParameterName::RDotDotI(ddi) => ddi.syntax().text_trimmed().to_string(),
            };
            self.scan.flow_state.bind(text);
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
    pub(super) fn scan_source_call(
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
                self.scan.attached_flow.push(pkg.clone());
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
            Some(ScanScope::Descent(bound)) => bound.binds(name),
            Some(ScanScope::Scope(scope)) => self.scope_binds_anywhere(scope, name),
            None => false,
        }
    }

    fn scan_scope_is_global(&self) -> bool {
        match self.scan_scope() {
            Some(ScanScope::Scope(scope)) => matches!(self.scopes[scope].kind, ScopeKind::File),
            Some(ScanScope::Descent(_)) => false,
            None => true,
        }
    }

    fn scan_scope(&self) -> Option<ScanScope<'_>> {
        if let Some(bound) = self.scan.eager_descent.open.last() {
            return Some(ScanScope::Descent(bound));
        }

        let scope = match self.scopes[self.current_scope].kind {
            ScopeKind::Nse(EvalEnv::Current, EvalTiming::Lazy) => self.definition_owner()?,
            _ => self.current_scope,
        };
        Some(ScanScope::Scope(scope))
    }

    /// Record the names a child scope (function body, NSE argument) about to be
    /// created at `range` inherits from its ancestors, to seed the child's scan
    /// in `begin_scan`. Called during the scan, where `flow_state` is the
    /// parent's flow-precise state at the child's definition point (already
    /// carrying the parent's own inherited ancestors, so the child inherits
    /// transitively).
    pub(super) fn record_enclosing_flow(&mut self, range: TextRange) {
        self.scan
            .enclosing_flow
            .insert(range, self.scan.flow_state.snapshot());
    }

    /// Record a binding in the scan's flow state.
    ///
    /// The flow-precise `flow_state` always learns the name, so a
    /// later callee in this scope sees it shadowed. The bound names only get it
    /// when the current scope owns it. A `Current + Lazy` scope routes its defs
    /// to the owner, so the name is added to the owner's bound names instead, the
    /// same routing `add_definition_to_owner` does during the walk.
    pub(super) fn record_binding(&mut self, name: String, range: TextRange) {
        self.record_owner_name(name.clone(), range);
        self.scan.flow_state.bind(name);
    }

    /// Route a binding NAME into its owner scope's bound names, matching
    /// `add_definition`'s routing. When a descent is open the name goes to the
    /// descent top, which is always an eager `Nested` body scanned inline and so
    /// owns its bindings. Otherwise a `Current + Lazy` scope routes to
    /// `definition_owner()` and every other scope owns its bindings.
    ///
    /// Split from `record_binding` so `scan_lazy_owner_bindings` can add
    /// a deferred body's names to the owner's bound names without also marking them
    /// bound in `flow_state` (see that helper for why).
    pub(super) fn record_owner_name(&mut self, name: String, range: TextRange) {
        if let Some(bound) = self.scan.eager_descent.open.last_mut() {
            bound.add(name, range);
            return;
        }

        if let Some(target) = match self.scopes[self.current_scope].kind {
            ScopeKind::Nse(EvalEnv::Current, EvalTiming::Lazy) => self.definition_owner(),
            _ => Some(self.current_scope),
        } {
            self.scan.bound_names[target].add(name, range);
        }
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

/// Backs a [`CallContext`]'s [`ScopeBindings`] with the builder's live scope
/// state, so an effect handler (`substitute`) can query bindings during the
/// scan without reaching into the builder directly.
///
/// [`CallContext`]: crate::effects::CallContext
pub(super) struct ScanBindings<'a, R: ImportsResolver> {
    pub(super) builder: &'a SemanticIndexBuilder<R>,
}

impl<R: ImportsResolver> ScopeBindings for ScanBindings<'_, R> {
    fn is_bound(&self, name: &str, inherits: bool) -> bool {
        if inherits {
            // The scan's `flow_state` carries the current scope's bindings plus
            // the inherited eager environment seeded at `begin_scan`, so it's
            // the lexical answer.
            return self.builder.scan.flow_state.is_bound(name);
        }
        self.builder.scan_scope_binds(name)
    }

    fn is_global_scope(&self) -> bool {
        self.builder.scan_scope_is_global()
    }
}

/// The scan's flow-precise binding state: which names are bound at the current
/// point of the current scan unit, in flow order.
///
/// It's the scan's own flow state, a coarse variant of the walk's use-def map,
/// which isn't built yet. It answers one question, "is this name bound here?",
/// so the scan can tell whether a callee is shadowed at each call and decide
/// whether a call is NSE. It tracks only eager bindings, and it is allowed to
/// stay coarse: `merge()` unions the two sides of an `if`, so that a single
/// branch marks a name as bound.
#[derive(Clone, Default)]
pub(super) struct FlowState {
    bound: FxHashSet<String>,
}

impl FlowState {
    /// Save the current state, to rewind to or to seed a child scan unit from.
    pub(super) fn snapshot(&self) -> FlowState {
        self.clone()
    }

    /// Rewind to `snapshot`, dropping any bindings recorded since it was taken.
    pub(super) fn restore(&mut self, snapshot: FlowState) {
        *self = snapshot;
    }

    /// Union `snapshot` in, so a name reads as bound here if it was bound on
    /// either path. This is the `if`/`else` join.
    fn merge(&mut self, snapshot: FlowState) {
        self.bound.extend(snapshot.bound);
    }

    /// Record `name` as bound from here on.
    fn bind(&mut self, name: String) {
        self.bound.insert(name);
    }

    /// Whether `name` is bound at the current point.
    pub(super) fn is_bound(&self, name: &str) -> bool {
        self.bound.contains(name)
    }

    /// Drop all bindings, to start a fresh scan unit (see `begin_scan()`).
    fn clear(&mut self) {
        self.bound.clear();
    }
}

/// Tracks eager `Nested` NSE bodies scanned inline during the scan.
///
/// An eager `Nested` body like `local()` runs immediately at its call site, so
/// we scan it inline instead of deferring it to the walk. `open` is the stack
/// of bodies being scanned right now, with the innermost on top.
/// `record_owner_name()` routes a binding to the top so names land on the body
/// that owns them. When a descent finishes, its names move to `pending`, keyed
/// by the body's range.
///
/// `pending` is keyed by range rather than written straight into
/// `bound_names[scope]` because the body's arena scope doesn't exist yet: the
/// walk allocates scopes in preorder, and allocating one mid-scan would break
/// the `Scope::descendants` invariant. The range is the body's pre-arena
/// identity until the walk pushes its scope.
///
/// Once the walk pushes that scope, it installs the pending names into it
/// instead of re-scanning. It does this before collecting the body, because a
/// lazy child inside (a function or lazy NSE body) runs later than the walk
/// reaches it, so it can reference a binding defined further down this scope.
/// Resolving that name checks whether an enclosing scope binds it
/// (`scope_binds_anywhere()`), and the walk hasn't recorded that binding yet, so
/// the scan-populated bound set has to be complete up front. That's the reason
/// the scan collects bound names ahead of the walk at all.
#[derive(Default)]
pub(super) struct EagerNestedDescent {
    pub(super) open: Vec<BoundNames>,
    pub(super) pending: FxHashMap<TextRange, BoundNames>,
}

/// All definitions in a scope, collected by the scan pass before the
/// walk. Skips child-scope bodies (nested functions and `Nested` NSE bodies).
///
/// Keeps each name's earliest binding site in scan order, which is source
/// order within the scope. A name bound several times reads as bound
/// throughout, and this earliest site is what the lazy-shadow diagnostic points
/// at, once `is_lazily_shadowed` has picked the nearest binding ancestor.
pub(super) struct BoundNames {
    by_name: FxHashMap<String, TextRange>,
}

impl BoundNames {
    pub(super) fn new() -> Self {
        Self {
            by_name: FxHashMap::default(),
        }
    }

    fn add(&mut self, name: String, range: TextRange) {
        self.by_name.entry(name).or_insert(range);
    }

    pub(super) fn binds(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    pub(super) fn binding_range(&self, name: &str) -> Option<TextRange> {
        self.by_name.get(name).copied()
    }
}

/// A scope as the scan sees it. A `local()` body scanned inline has no arena
/// scope yet and its bindings are stored in the staging [`EagerNestedDescent`].
/// Every other scope is materialized in the arena. [`scan_scope`] resolves
/// which one is the current evaluation frame.
///
/// [`scan_scope`]: SemanticIndexBuilder::scan_scope
enum ScanScope<'a> {
    Descent(&'a BoundNames),
    Scope(ScopeId),
}
