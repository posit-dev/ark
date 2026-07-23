use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
use aether_syntax::RArgument;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use biome_rowan::AstPtr;
use biome_rowan::AstSeparatedList;
// Re-exported so consumers building an `AssignBinding` (custom `AssignHandler`s)
// can name the `name_expr` field's type without depending on oak_core directly.
pub use oak_core::range::RangedAstPtr;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;

/// Per-package tables of which functions carry effects. Private data behind the
/// `lookup`/`annotates` query API below.
mod contrib;

/// Effects of a call, resolved against the call site.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    /// Per-argument evaluation effects, resolved against the call and aligned
    /// 1:1 with its arguments. `None` at a slot means a plain (standard-eval)
    /// argument.
    pub arguments: Option<ResolvedArgumentEffects>,
    /// Attach a package
    pub attach: Option<String>,
    /// Source one or more files. A vector so a collation-style callee can name
    /// several; base `source` resolves to one.
    pub source: Option<Vec<String>>,
    /// Bind one or more names in the current scope (`assign("x", value)`). A
    /// vector so a multi-binding callee stays expressible; base `assign` and
    /// `delayedAssign` resolve to one.
    pub assign: Option<Vec<AssignBinding>>,
}

/// One name an assign call binds, with the syntax handles its consumers need.
/// - The bound name feeds the symbol table.
/// - `name_expr` anchors the goto target and carries a trimmed range that can
///   be matched against a cursor (e.g. for goto/rename).
/// - `value_expr` is what a type checker infers the binding's type from (`None`
///   with no value argument).
#[derive(Debug, Clone)]
pub struct AssignBinding {
    pub name: String,
    pub name_expr: RangedAstPtr<AnyRExpression>,
    pub value_expr: Option<AstPtr<AnyRExpression>>,
}

/// The handlers that compute a function's effects.
#[derive(Debug, Clone, Copy)]
pub struct EffectsHandlers {
    pub arguments: Option<&'static dyn EffectHandler<Output = ResolvedArgumentEffects>>,
    pub attach: Option<&'static dyn EffectHandler<Output = String>>,
    pub source: Option<&'static dyn EffectHandler<Output = Vec<String>>>,
    pub assign: Option<&'static dyn AssignHandler>,
}

/// Look up the effect handlers of a `(package, function)` pair.
pub fn lookup(package: &str, function: &str) -> Option<&'static EffectsHandlers> {
    contrib::REGISTRY
        .iter()
        .flat_map(|entries| entries.iter())
        .find(|entry| entry.package == package && entry.function == function)
        .map(|entry| &entry.effects)
}

/// Whether any registry entry annotates `name`. This is the bare-callee front
/// gate: an unannotated name can't resolve to an effect no matter which provider
/// wins, so recognition skips resolution entirely.
pub fn annotates(name: &str) -> bool {
    contrib::REGISTRY
        .iter()
        .flat_map(|entries| entries.iter())
        .any(|entry| entry.function == name)
}

/// Resolver for an effect of a call.
///
/// The single interface behind every effect kind (NSE, attach, source).
///
/// Handlers are contributed statically for now (a `&'static dyn` in the
/// registry), so the trait is `Sync`, which every registry `static` needs.
pub trait EffectHandler: std::fmt::Debug + Sync {
    type Output;

    /// Resolve this effect for `call`, or `None` when the call isn't in a shape
    /// this handler recognizes.
    ///
    /// `ctx` provides semantic resolution, e.g. resolve an argument to a
    /// statically known string or boolean.
    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<Self::Output>;
}

/// Where an effect is invoked. Most effects are only ever calls but an Assign
/// effect can also be a binding operator (`x %<>% f`). [`AssignHandler`] takes
/// this to disambiguate rather than a bare call.
pub enum EffectSite<'a> {
    Call(&'a RCall),
    Operator(&'a RBinaryExpression),
}

/// Resolver for an assign-like effect.
///
/// Separate from [`EffectHandler`] because an assign has two invocation shapes,
/// a call (`assign("x", v)`) and a binding operator (`x %<>% f`).
///
/// Contributed statically like [`EffectHandler`], so it's `Sync` for the
/// registry `static`s.
pub trait AssignHandler: std::fmt::Debug + Sync {
    fn resolve(&self, site: EffectSite, ctx: &CallContext<'_>) -> Option<Vec<AssignBinding>>;
}

/// Scope state a handler needs that the call syntax alone can't answer, backed
/// by the builder's flow-precise binding tables.
///
/// `substitute` uses this to tell which symbols in its argument name a binding
/// in the current scope (so they resolve here, against substitute's env) from
/// those that stay quoted (so they resolve wherever the result is later
/// evaluated).
pub trait ScopeBindings {
    /// Whether `name` is bound in the current scope. With `inherits`, also
    /// counts bindings inherited from enclosing scopes, mirroring R's
    /// `get(..., inherits=)`.
    fn is_bound(&self, name: &str, inherits: bool) -> bool;

    /// Whether the current scope is the global (file) scope. R's `substitute`
    /// substitutes nothing in the global environment, so a handler falls back to
    /// a plain quote there.
    fn is_global_scope(&self) -> bool;
}

/// Context for effect handlers.
///
/// Allows querying the properties or static values of arguments, and the
/// binding state of the surrounding scope.
#[derive(Default)]
pub struct CallContext<'a> {
    bindings: Option<&'a dyn ScopeBindings>,
}

impl<'a> CallContext<'a> {
    /// A context backed by the builder's scope state, for handlers that query
    /// bindings (`substitute`).
    pub fn with_bindings(bindings: &'a dyn ScopeBindings) -> Self {
        Self {
            bindings: Some(bindings),
        }
    }

    /// Whether `name` is bound in the current scope (see
    /// [`ScopeBindings::is_bound`]). Without a bindings backing (a [`Default`]
    /// context) we can't tell, so we answer "unbound", the choice that leaves a
    /// symbol quoted rather than treating it as a use.
    pub fn is_bound(&self, name: &str, inherits: bool) -> bool {
        self.bindings
            .is_some_and(|bindings| bindings.is_bound(name, inherits))
    }

    /// Whether the current scope is the global (file) scope (see
    /// [`ScopeBindings::is_global_scope`]). Without a bindings backing (a
    /// [`Default`] context) we assume global, so `substitute` degrades to a
    /// plain quote (its no-substitution behaviour).
    pub fn current_scope_is_global(&self) -> bool {
        self.bindings
            .is_none_or(|bindings| bindings.is_global_scope())
    }

    /// Match `call`'s arguments to `formals`, returning for each call argument
    /// in order the index into `formals` it bound to. Named arguments match
    /// first, then the rest fill by position.
    ///
    /// A stopgap: without the callee's full formal list, a positional argument
    /// only binds a formal declared at that exact position.
    pub fn match_arguments(&self, call: &RCall, formals: &[Formal]) -> Vec<Option<usize>> {
        let Ok(args) = call.arguments() else {
            return Vec::new();
        };
        let items = args.items();

        let arg_count = items.iter().count();
        let mut matched: Vec<Option<usize>> = vec![None; arg_count];
        let mut consumed = vec![false; formals.len()];

        // Named pass
        for (i, item) in items.iter().enumerate() {
            let Ok(arg) = item else { continue };
            if let Some(formal_idx) = match_named(&arg, formals, &consumed) {
                consumed[formal_idx] = true;
                matched[i] = Some(formal_idx);
            }
        }

        // Positional pass. Only unnamed args reach the match, and none of them
        // were set by the named pass, so no need to re-check `matched[i]`.
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
            if let Some(formal_idx) = match_positional(formals, position, &consumed) {
                consumed[formal_idx] = true;
                matched[i] = Some(formal_idx);
            }
            position += 1;
        }

        matched
    }

    /// Statically evaluate an argument's value expression to a string. `None`
    /// when it's dynamic.
    pub fn resolve_static_string(&self, value: &AnyRExpression) -> Option<String> {
        match value {
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
            // Static resolution of expressions is not implemented yet
            _ => None,
        }
    }

    /// Read a quoted name argument. E.g. the LHS of an Assign operator.
    pub fn resolve_quoted_symbol_or_string(&self, value: &AnyRExpression) -> Option<String> {
        match value {
            AnyRExpression::RIdentifier(ident) => Some(ident.name_text()),
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
            _ => None,
        }
    }

    /// Statically evaluate an argument's value expression to a bool.
    pub fn resolve_static_bool(&self, value: &AnyRExpression) -> Option<bool> {
        match value {
            AnyRExpression::RTrueExpression(_) => Some(true),
            AnyRExpression::RFalseExpression(_) => Some(false),
            // Static resolution of expressions is not implemented yet
            _ => None,
        }
    }
}

/// A formal a handler wants to locate in a call, by name and by its position in
/// the callee's signature.
///
/// TODO(nse): `position` is a stopgap that stems from our annotation registry
/// listing only its scoped formals. Once `match_arguments` is signature-aware
/// it gets the callee's full ordered formals, and this collapses to a list of
/// names where the index is the position.
pub struct Formal {
    pub name: &'static str,
    pub position: usize,
}

/// A call's resolved argument effects: for each argument in call order, the
/// effect it resolved to, or `None` for a plain (standard-eval) argument.
pub type ResolvedArgumentEffects = Vec<Option<ResolvedArgumentEffect>>;

/// The resolved, per-call effect of one argument. The builder consumes these.
#[derive(Debug, Clone)]
pub enum ResolvedArgumentEffect {
    /// Quote the argument, then evaluate it in `env`. `timing` says whether
    /// that happens eagerly at the call site (`evalq()`, `local()`) or later
    /// at an unknown time (`on_load()`, `reactive()`).
    EvalQ { env: EvalEnv, timing: EvalTiming },
    /// Captured unevaluated. `holes` are the sub-expressions that escape back to
    /// evaluation (e.g. bquote's `.()` contents), walked normally; everything
    /// else in the argument is inert. Empty for a plain `quote()`.
    Quote { holes: Vec<AnyRExpression> },
}

/// Declares how a function evaluates its annotated arguments, and serves as the
/// default [`EffectHandler`] for it by matching the declaration to a call.
#[derive(Debug, Clone, Copy)]
pub struct ArgumentsAnnotation {
    pub arguments: &'static [Argument],
}

/// A single annotated argument: its effect, plus where to find it in a call.
#[derive(Debug)]
pub struct Argument {
    pub name: &'static str,
    pub position: usize,
    pub effect: ArgumentEffect,
}

/// What static operation an argument's evaluation calls for, mirroring R's
/// evaluation model.
#[derive(Debug, Clone, Copy)]
pub enum ArgumentEffect {
    /// Quote the argument, then evaluate it in `env`. `timing` says whether
    /// that happens eagerly at the call site (`evalq()`, `local()`) or later
    /// at an unknown time (`on_load()`, `reactive()`).
    EvalQ { env: EvalEnv, timing: EvalTiming },
    /// Captured unevaluated, so its symbols are not uses and nothing in it runs.
    /// `quote`. A function that unquotes (`bquote()`, whose `.()` holes escape)
    /// can't be expressed statically, and must use a custom handler instead of
    /// this variant.
    Quote,
}

impl ArgumentEffect {
    fn resolve(self) -> ResolvedArgumentEffect {
        match self {
            ArgumentEffect::EvalQ { env, timing } => ResolvedArgumentEffect::EvalQ { env, timing },
            ArgumentEffect::Quote => ResolvedArgumentEffect::Quote { holes: Vec::new() },
        }
    }
}

impl EffectHandler for ArgumentsAnnotation {
    type Output = ResolvedArgumentEffects;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<ResolvedArgumentEffects> {
        let arguments = self.arguments;
        let formals: Vec<Formal> = arguments
            .iter()
            .map(|arg| Formal {
                name: arg.name,
                position: arg.position,
            })
            .collect();

        // The match yields a formal index per call argument
        let matched = ctx.match_arguments(call, &formals);
        Some(
            matched
                .into_iter()
                .map(|formal| formal.map(|i| arguments[i].effect.resolve()))
                .collect(),
        )
    }
}

/// Declares how a source function (`source()`) names the file it reads, and
/// serves as the default [`EffectHandler`] for it by pulling that path out of a
/// call.
#[derive(Debug, Clone, Copy)]
pub struct SourceAnnotation {
    /// Which positional argument holds the path, counting only unnamed
    /// arguments (0 for base `source`). Other source-like functions may put the
    /// path elsewhere, so it's configured per entry rather than assumed.
    pub position: usize,
}

impl EffectHandler for SourceAnnotation {
    type Output = Vec<String>;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<Vec<String>> {
        let args = call.arguments().ok()?;

        // The path is matched positionally among unnamed arguments rather than
        // through [`CallContext::match_arguments`], for two reasons. We need to
        // inspect the `local =` value to bail on non-static calls, which
        // argument matching doesn't do. And counting unnamed arguments is robust
        // to a named argument coming first (e.g. `source(echo = TRUE, "x.R")`),
        // which the call-position matching isn't yet. A named `file =` therefore
        // isn't recognized today.
        //
        // TODO(nse): once `match_arguments` is signature-aware (see `Formal`),
        // the leading-named-arg robustness comes for free and this scan could
        // fold onto it, keeping only the `local =` bail on top.
        let mut path: Option<String> = None;
        let mut positional = 0;

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
                            // Anything else (environment, non-statically
                            // resolvable expression) means the call isn't
                            // statically analyzable, so it's not recognized.
                            _ => return None,
                        }
                    }
                }
                continue;
            }

            if positional == self.position {
                path = arg
                    .value()
                    .and_then(|value| ctx.resolve_static_string(&value));
            }
            positional += 1;
        }

        path.map(|resolved| vec![resolved])
    }
}

/// Declares how an assign function (`assign()`, `delayedAssign()`) names the
/// variable it binds, and serves as the default [`EffectHandler`] for it by
/// pulling that name out of a call.
#[derive(Debug, Clone, Copy)]
pub struct AssignAnnotation {
    /// Which positional argument holds the bound name, counting only unnamed
    /// arguments (0 for base `assign`/`delayedAssign`).
    pub position: usize,
}

impl AssignHandler for AssignAnnotation {
    fn resolve(&self, site: EffectSite, ctx: &CallContext<'_>) -> Option<Vec<AssignBinding>> {
        let EffectSite::Call(call) = site else {
            return None;
        };
        let args = call.arguments().ok()?;

        // Matched positionally among unnamed arguments, same as `source`, so a
        // leading named argument doesn't shift the count and a named `x =` isn't
        // recognized. The value is the positional right after the name (base
        // `assign(x, value, ...)`).
        //
        // FIXME: A named `value =` isn't captured yet.
        // TODO(nse): Fold onto `match_arguments()` once it's signature-aware,
        // same as `source` (see `SourceAnnotation::resolve`), keeping only the
        // `envir`/`pos` bail and the value-after-name read on top.
        let mut name: Option<(String, RangedAstPtr<AnyRExpression>)> = None;
        let mut value_expr: Option<AstPtr<AnyRExpression>> = None;
        let mut positional = 0;

        for item in args.items().iter() {
            let Ok(arg) = item else { continue };

            if let Some(name_clause) = arg.name_clause() {
                let Ok(AnyRArgumentName::RIdentifier(name_ident)) = name_clause.name() else {
                    continue;
                };

                // An explicit target environment means the binding lands
                // somewhere other than the current scope, so it isn't a fact we
                // can record here. In the future, we could statically recognise
                // some environment selectors like `parent.frame()`.
                if matches!(name_ident.name_text().as_str(), "envir" | "pos") {
                    return None;
                }
                continue;
            }

            if positional == self.position {
                if let Some(value) = arg.value() {
                    if let Some(resolved) = ctx.resolve_static_string(&value) {
                        name = Some((resolved, RangedAstPtr::new(&value)));
                    }
                }
            } else if positional == self.position + 1 {
                value_expr = arg.value().map(|value| AstPtr::new(&value));
            }
            positional += 1;
        }

        let (name, name_expr) = name?;
        Some(vec![AssignBinding {
            name,
            name_expr,
            value_expr,
        }])
    }
}

/// Handler for a binding operator (`x %<>% f()`, `x %<~% expr`, `x := expr`).
///
/// The operator captures its LHS unevaluated.
#[derive(Debug, Clone, Copy)]
pub struct BindingOperatorHandler;

impl AssignHandler for BindingOperatorHandler {
    fn resolve(&self, site: EffectSite, ctx: &CallContext<'_>) -> Option<Vec<AssignBinding>> {
        let EffectSite::Operator(bin) = site else {
            return None;
        };
        let left = bin.left().ok()?;
        let right = bin.right().ok()?;

        let name = ctx.resolve_quoted_symbol_or_string(&left)?;

        Some(vec![AssignBinding {
            name,
            name_expr: RangedAstPtr::new(&left),
            value_expr: Some(AstPtr::new(&right)),
        }])
    }
}

/// Match a named argument against `formals`. Returns the index of the matched
/// formal.
///
/// Should we do partial argument matching? Or rely on partial matching being linted?
fn match_named(arg: &RArgument, formals: &[Formal], consumed: &[bool]) -> Option<usize> {
    let clause = arg.name_clause()?;
    let name = clause.name().ok()?;
    let name_text = match &name {
        AnyRArgumentName::RIdentifier(ident) => ident.name_text(),
        AnyRArgumentName::RStringValue(s) => s.string_text()?,
        _ => return None,
    };
    formals
        .iter()
        .enumerate()
        .find(|(i, formal)| !consumed[*i] && formal.name == name_text.as_str())
        .map(|(i, _)| i)
}

/// Match an unnamed argument at `position` against `formals`. Returns the index
/// of the matched formal.
///
/// FIXME: This matches positionally on call-site position only: an unnamed
/// argument at position N matches a formal declared at position N. It doesn't
/// replicate R's full matching, where named arguments are pulled out first and
/// the rest fill the remaining formals in order. So `test_that({ ... }, desc =
/// "d")`, with the block at position 0 but the `code` formal at position 1,
/// won't match. Good enough without the callee's formal list; revisit if it
/// misses real cases.
fn match_positional(formals: &[Formal], position: usize, consumed: &[bool]) -> Option<usize> {
    formals
        .iter()
        .enumerate()
        .find(|(i, formal)| !consumed[*i] && formal.position == position)
        .map(|(i, _)| i)
}
