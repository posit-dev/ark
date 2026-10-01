use std::sync::LazyLock;

use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
use aether_syntax::RArgument;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use biome_rowan::AstPtr;
use biome_rowan::AstSeparatedList;
use biome_rowan::TextRange;
// Re-exported so consumers building an `AssignBinding` (custom `AssignHandler`s)
// can name the `name_expr` field's type without depending on oak_core directly.
pub use oak_core::range::RangedAstPtr;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;
use rustc_hash::FxHashMap;

use crate::semantic_index::AmbiguityReason;
use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;

/// Per-package tables of which functions carry effects. Private data behind the
/// `lookup`/`annotates` query API below.
mod contrib;
mod value;
mod value_eval;

pub use value::StaticValue;
pub use value_eval::CalleeImport;
pub use value_eval::ValueHandler;

/// Registry entries keyed by function name so they can be queried by `lookup()`
/// (package plus function) and `annotates()` (function only) probe on. Entries
/// for a name carried by several packages (e.g. `defer()` in both withr and
/// rlang) are kept in registry order, so `lookup` breaks a tie the same way a
/// scan of `REGISTRY` would.
static INDEX: LazyLock<FxHashMap<&'static str, Vec<(&'static str, &'static FunctionHandlers)>>> =
    LazyLock::new(|| {
        let mut index: FxHashMap<&'static str, Vec<(&'static str, &'static FunctionHandlers)>> =
            FxHashMap::default();
        for package in contrib::REGISTRY {
            for entry in package.functions {
                index
                    .entry(entry.function)
                    .or_default()
                    .push((package.name, &entry.handlers));
            }
        }
        index
    });

/// Effects of a call, resolved against the call site.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    /// Per-argument evaluation effects, resolved against the call and aligned
    /// 1:1 with its arguments. `None` at a slot means a plain (standard-eval)
    /// argument.
    pub arguments: Option<ResolvedArgumentEffects>,
    /// Attach a package
    pub attach: Option<String>,
    /// Source one or more paths. A vector so a collation-style callee can name
    /// several; base `source` resolves to one.
    pub source: Option<Vec<SourcePath>>,
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
/// - `target` tells the walk whether the bound name is also read here.
#[derive(Debug, Clone)]
pub struct AssignBinding {
    pub name: String,
    pub name_expr: RangedAstPtr<AnyRExpression>,
    pub value_expr: Option<AstPtr<AnyRExpression>>,
    pub target: TargetAccess,
}

/// Effect handling and static evaluation are independent. For example, `c()`
/// has a value handler but no effects.
#[derive(Debug, Clone, Copy)]
pub struct FunctionHandlers {
    pub effects: EffectsHandlers,
    pub value: Option<&'static dyn ValueHandler>,
}

impl FunctionHandlers {
    pub const fn with_effects(effects: EffectsHandlers) -> Self {
        Self {
            effects,
            value: None,
        }
    }

    pub const fn with_value(value: &'static dyn ValueHandler) -> Self {
        Self {
            effects: EffectsHandlers::EMPTY,
            value: Some(value),
        }
    }

    pub fn has_effects(&self) -> bool {
        !self.effects.is_empty()
    }

    pub fn has_value(&self) -> bool {
        self.value.is_some()
    }
}

/// The handlers that compute a function's effects.
#[derive(Debug, Clone, Copy)]
pub struct EffectsHandlers {
    pub arguments: Option<&'static dyn EffectHandler<Output = ResolvedArgumentEffects>>,
    pub attach: Option<&'static dyn EffectHandler<Output = String>>,
    pub source: Option<&'static dyn EffectHandler<Output = Vec<SourcePath>>>,
    pub assign: Option<&'static dyn AssignHandler>,
}

impl EffectsHandlers {
    pub const EMPTY: EffectsHandlers = EffectsHandlers {
        arguments: None,
        attach: None,
        source: None,
        assign: None,
    };

    pub fn is_empty(&self) -> bool {
        self.arguments.is_none() &&
            self.attach.is_none() &&
            self.source.is_none() &&
            self.assign.is_none()
    }
}

/// The source of a callee resolution, including lookups with no known handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalleeOrigin {
    /// A binding in the file's own scopes, visible at the call site.
    Local,
    /// The name is locally unbound, so the imports resolver decides it: the
    /// file's attaches, what sourcing files bring in, and base. This includes
    /// lookups that found no handler.
    Import,
    /// `pkg::fn` or `pkg:::fn`, which no binding or attach can change.
    Qualified,
    /// Callees such as `f()()` or `x$f()` require runtime evaluation, so
    /// name lookup cannot provide handlers.
    Dynamic,
}

/// Flow uncertainty is retained alongside the handlers so each consumer can
/// report only ambiguity relevant to the handler it uses.
#[derive(Debug, Clone)]
pub struct CalleeResolution {
    pub origin: CalleeOrigin,
    pub handlers: Option<FunctionHandlers>,
    uncertainty: Option<CalleeUncertainty>,
}

/// Import uncertainty is classified by whether handlers were found.
/// Only successful lookups are checked for lazy shadowing, and only lookups
/// without handlers are checked for dropped attaches.
#[derive(Debug, Clone)]
pub(crate) enum CalleeUncertainty {
    /// A binding in an enclosing scope, whose timing relative to this lazy
    /// body is unknown, could shadow the handlers that were found.
    LazyShadow { overwrite_range: TextRange },
    /// Attaches dropped at a branch or loop join could supply missing handlers.
    /// Candidates are in reverse attach order, not a reconstructed runtime
    /// search path. Each consumer reports the first candidate with the handler
    /// it needs, even if a newer candidate binds the name without that handler.
    ConditionalAttach(Vec<DroppedAttach>),
}

#[derive(Debug, Clone)]
pub(crate) struct DroppedAttach {
    pub(crate) package: String,
    pub(crate) attach_range: TextRange,
    pub(crate) handlers: FunctionHandlers,
}

impl CalleeResolution {
    pub(crate) fn new(
        origin: CalleeOrigin,
        handlers: Option<FunctionHandlers>,
        uncertainty: Option<CalleeUncertainty>,
    ) -> Self {
        Self {
            origin,
            handlers,
            uncertainty,
        }
    }

    /// Construct a resolution with no recorded flow uncertainty. This lets
    /// [`ScopeContext`] implementations outside the scan return a resolution
    /// without access to the scanner's uncertainty tracking.
    pub fn settled(origin: CalleeOrigin, handlers: Option<FunctionHandlers>) -> Self {
        Self::new(origin, handlers, None)
    }

    /// Report only uncertainty involving handlers accepted by `uses`, such as
    /// [`FunctionHandlers::has_value()`]. A value-only handler should not cause
    /// an effect diagnostic, and an effect-only handler should not cause a
    /// static-value diagnostic.
    pub fn ambiguity(&self, uses: fn(&FunctionHandlers) -> bool) -> Option<AmbiguityReason> {
        match self.uncertainty.as_ref()? {
            CalleeUncertainty::LazyShadow { overwrite_range } => self
                .handlers
                .as_ref()
                .is_some_and(uses)
                .then_some(AmbiguityReason::LazyShadow {
                    overwrite_range: *overwrite_range,
                }),
            CalleeUncertainty::ConditionalAttach(dropped) => dropped
                .iter()
                .find(|attach| uses(&attach.handlers))
                .map(|attach| AmbiguityReason::ConditionalAttach {
                    package: attach.package.clone(),
                    attach_range: attach.attach_range,
                }),
        }
    }
}

/// Look up the handlers of a `(package, function)` pair.
pub fn lookup(package: &str, function: &str) -> Option<&'static FunctionHandlers> {
    INDEX
        .get(function)?
        .iter()
        .find(|(entry_package, _)| *entry_package == package)
        .map(|(_, effects)| *effects)
}

/// Whether any registry entry annotates `name`. This is the bare-callee front
/// gate: an unannotated name can't resolve to an effect no matter which provider
/// wins, so recognition skips resolution entirely.
pub fn annotates(name: &str) -> bool {
    INDEX.contains_key(name)
}

/// HACK: This matches a `sourceDir()` call syntactically. See `?source` for the
/// definition of `sourceDir()` that people copy around:
/// https://github.com/search?q=sourceDir+language%3AR&type=code
/// This is a stopgap workaround until we can infer source effects around a
/// `list.files()` loop.
///
/// The copied `sourceDir()` idiom leaves `list.files()` at its
/// `recursive = FALSE` default, so nested scripts are excluded.
pub fn source_dir_idiom(name: &str) -> Option<&'static FunctionHandlers> {
    static SOURCE_DIR: FunctionHandlers = FunctionHandlers::with_effects(EffectsHandlers {
        source: Some(&SourceAnnotation {
            formals: &["path"],
            path: "path",
            target: SourceTarget::Dir(DirWalk::Shallow),
            default_path: None,
        }),
        ..EffectsHandlers::EMPTY
    });

    (name == "sourceDir").then_some(&SOURCE_DIR)
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
    fn resolve(&self, call: &RCall, ctx: &mut CallContext<'_>) -> Option<Self::Output>;
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
    fn resolve(&self, site: EffectSite, ctx: &mut CallContext<'_>) -> Option<Vec<AssignBinding>>;
}

/// Scope state a handler needs that the call syntax alone can't answer, backed
/// by the builder's flow-precise binding tables.
///
/// `substitute` uses this to tell which symbols in its argument name a binding
/// in the current scope (so they resolve here, against substitute's env) from
/// those that stay quoted (so they resolve wherever the result is later
/// evaluated).
pub trait ScopeContext {
    /// Whether `name` is bound in the current scope. With `inherits`, also
    /// counts bindings inherited from enclosing scopes, mirroring R's
    /// `get(..., inherits=)`.
    fn is_bound(&self, name: &str, inherits: bool) -> bool;

    /// Whether the current scope is the global (file) scope. R's `substitute`
    /// substitutes nothing in the global environment, so a handler falls back to
    /// a plain quote there.
    fn is_global(&self) -> bool;

    /// Use live local bindings, then imports, for nested calls, so a local
    /// definition can shadow a registered value handler.
    fn resolve_callee(&mut self, call: &RCall) -> CalleeResolution;
}

/// Whether an assign effect reads its target before writing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetAccess {
    /// Writes the target without reading it, as in `x <- value`.
    Write,
    /// Reads the target before rebinding it. `x %<>% f()` expands to
    /// `x <- x %>% f()`, so `x` is a use and a definition.
    ReadWrite,
}

/// Gives effect handlers access to scope bindings and static argument values.
pub struct CallContext<'a> {
    scope: &'a mut dyn ScopeContext,
    /// Collected by static evaluation and left to the consumer to report, so
    /// requesting a value has no diagnostic side effect.
    callee_imports: Vec<CalleeImport>,
}

impl<'a> CallContext<'a> {
    pub fn new(scope: &'a mut dyn ScopeContext) -> Self {
        Self {
            scope,
            callee_imports: Vec::new(),
        }
    }

    /// The imported callees static evaluation consulted, one per call site, in
    /// the order they were first consulted.
    pub fn into_callee_imports(self) -> Vec<CalleeImport> {
        self.callee_imports
    }

    /// Whether `name` is bound in the current scope (see
    /// [`ScopeContext::is_bound`]).
    pub fn is_bound(&self, name: &str, inherits: bool) -> bool {
        self.scope.is_bound(name, inherits)
    }

    /// Whether the current scope is the global (file) scope (see
    /// [`ScopeContext::is_global`]).
    pub fn current_scope_is_global(&self) -> bool {
        self.scope.is_global()
    }
}

/// Include formals in signature order through every argument the handler reads,
/// including earlier slots so unnamed arguments bind correctly. A `"..."` slot
/// ends positional matching. Remaining unnamed arguments belong to `...`, and
/// later formals match only by exact name, as in R.
pub type Formals = &'static [&'static str];

/// A call's arguments indexed by the formals they match.
pub struct BoundArguments {
    formals: Formals,
    /// One entry per call argument, in call order: the formal it matched and its
    /// value expression.
    bound: Vec<(Option<usize>, Option<AnyRExpression>)>,
}

impl BoundArguments {
    /// Returns `None` if multiple named arguments match the same declared formal
    /// or the call has no argument list. R rejects duplicate matches.
    pub fn new(call: &RCall, formals: Formals) -> Option<Self> {
        let matched = match_arguments(call, formals)?;
        let values: Vec<Option<AnyRExpression>> = call
            .arguments()
            .ok()?
            .items()
            .iter()
            .map(|item| item.ok().and_then(|arg| arg.value()))
            .collect();
        Some(Self {
            formals,
            bound: matched.into_iter().zip(values).collect(),
        })
    }

    /// The expression bound to `formal`. Returns `None` when the call uses the
    /// default or the handler did not declare the formal.
    pub fn get(&self, formal: &str) -> Option<&AnyRExpression> {
        let formal_idx = self.formals.iter().position(|name| *name == formal)?;
        self.bound
            .iter()
            .find(|(matched_idx, _)| *matched_idx == Some(formal_idx))?
            .1
            .as_ref()
    }

    /// Returns each call argument's matched formal and expression in call order.
    pub fn arguments(&self) -> impl Iterator<Item = (Option<&str>, Option<&AnyRExpression>)> + '_ {
        self.bound
            .iter()
            .map(|(matched_idx, value)| (matched_idx.map(|idx| self.formals[idx]), value.as_ref()))
    }

    pub fn len(&self) -> usize {
        self.bound.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }
}

/// Exact named matches take priority over positional matches. Unnamed arguments
/// fill remaining slots in signature order, stopping at `"..."`.
///
/// Each result entry is a formal index or `None` for an unmatched argument,
/// including arguments belonging to `...`. Unmatched names may repeat.
/// The entire result is `None` if multiple named arguments match the same
/// declared formal or the call has no argument list.
fn match_arguments(call: &RCall, formals: Formals) -> Option<Vec<Option<usize>>> {
    let items = call.arguments().ok()?.items();

    let arg_count = items.iter().count();
    let mut matched: Vec<Option<usize>> = vec![None; arg_count];
    let mut consumed = vec![false; formals.len()];

    for (i, item) in items.iter().enumerate() {
        let Ok(arg) = item else { continue };
        let Some(formal_idx) = match_named(&arg, formals) else {
            continue;
        };
        // TODO: Diagnose duplicate formal matches, which R rejects. Handlers
        // have no diagnostic channel, and `Formals` may omit unread formals,
        // so a general lint needs the resolved callee's full signature.
        if consumed[formal_idx] {
            return None;
        }
        consumed[formal_idx] = true;
        matched[i] = Some(formal_idx);
    }

    // Unnamed arguments cannot have a named match, so `matched[i]` needs no check.
    let positional = formals
        .iter()
        .position(|formal| *formal == "...")
        .unwrap_or(formals.len());
    let mut next_slot = 0usize;
    for (i, item) in items.iter().enumerate() {
        let Ok(arg) = item else { continue };
        if arg.name_clause().is_some() {
            continue;
        }
        while next_slot < positional && consumed[next_slot] {
            next_slot += 1;
        }
        let Some(formal_idx) = (next_slot < positional).then_some(next_slot) else {
            continue;
        };
        consumed[formal_idx] = true;
        matched[i] = Some(formal_idx);
        next_slot += 1;
    }

    Some(matched)
}

/// Only exact names match. Partial argument matching is not supported.
///
/// TODO: Decide whether to support partial matching or rely on linting it.
fn match_named(arg: &RArgument, formals: Formals) -> Option<usize> {
    let clause = arg.name_clause()?;
    let name = clause.name().ok()?;
    let name_text = match &name {
        AnyRArgumentName::RIdentifier(ident) => ident.name_text(),
        AnyRArgumentName::RStringValue(s) => s.string_text()?,
        _ => return None,
    };
    formals
        .iter()
        .position(|formal| *formal != "..." && *formal == name_text.as_str())
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
    pub formals: Formals,
    pub arguments: &'static [Argument],
}

#[derive(Debug)]
pub struct Argument {
    pub name: &'static str,
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

    fn resolve(&self, call: &RCall, _ctx: &mut CallContext<'_>) -> Option<ResolvedArgumentEffects> {
        let Some(bound) = BoundArguments::new(call, self.formals) else {
            return Some(inert_argument_effects(call));
        };
        Some(
            bound
                .arguments()
                .map(|(formal, _)| {
                    let formal = formal?;
                    self.arguments
                        .iter()
                        .find(|argument| argument.name == formal)
                        .map(|argument| argument.effect.resolve())
                })
                .collect(),
        )
    }
}

/// Effects for a call whose arguments [`BoundArguments::new()`] can't match.
/// R rejects such a call before evaluating any argument, so every argument
/// stays inert. Treating them as plain arguments instead would scan bindings
/// that never happen, such as the `c <- identity` in
/// `substitute(expr = { c <- identity }, expr = NULL)`.
pub(crate) fn inert_argument_effects(call: &RCall) -> ResolvedArgumentEffects {
    let count = match call.arguments() {
        Ok(args) => args.items().iter().count(),
        Err(_) => 0,
    };
    vec![Some(ResolvedArgumentEffect::Quote { holes: Vec::new() }); count]
}

/// A path a source call names, and what that path points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcePath {
    pub path: String,
    pub target: SourceTarget,
}

/// What a source function's path argument points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTarget {
    /// A single file, as base `source()` takes.
    File,
    /// R files in a directory. [`DirWalk`] determines whether descendants count.
    Dir(DirWalk),
    /// A file or directory. `source()` takes only files, while
    /// `targets::tar_source()` takes both.
    FileOrDir(DirWalk),
}

/// Controls whether directory source targets include descendants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DirWalk {
    /// Direct children only, matching `list.files()` without `recursive = TRUE`.
    Shallow,
    /// Every R file below the directory, matching `list.files(recursive = TRUE)`.
    Recursive,
}

/// Declares how a source function (`source()`) names what it reads, and serves
/// as the default [`EffectHandler`] for it by pulling that path out of a call.
#[derive(Debug, Clone, Copy)]
pub struct SourceAnnotation {
    pub formals: Formals,
    pub path: &'static str,
    /// Whether that argument names a file or a directory.
    pub target: SourceTarget,
    /// Default path if no argument is suppolied (`tar_source()` defaults to
    /// `files = "R"`).
    pub default_path: Option<&'static str>,
}

impl EffectHandler for SourceAnnotation {
    type Output = Vec<SourcePath>;

    fn resolve(&self, call: &RCall, ctx: &mut CallContext<'_>) -> Option<Vec<SourcePath>> {
        let bound = BoundArguments::new(call, self.formals)?;

        // Only a statically known `local` makes the source scope known.
        if let Some(local) = bound.get("local") {
            ctx.resolve_static_bool(local)?;
        }

        let paths = match bound.get(self.path) {
            // An explicit dynamic path suppresses the default.
            Some(value) => ctx.resolve_static_character(value)?,
            None => vec![self.default_path?.to_string()],
        };

        // `source()` errors on a path vector unless it has exactly one element.
        if self.target == SourceTarget::File && paths.len() != 1 {
            return None;
        }

        Some(
            paths
                .into_iter()
                .map(|path| SourcePath {
                    path,
                    target: self.target,
                })
                .collect(),
        )
    }
}

/// Declares how an assign function (`assign()`, `delayedAssign()`) names the
/// variable it binds, and serves as the default [`EffectHandler`] for it by
/// pulling that name out of a call.
#[derive(Debug, Clone, Copy)]
pub struct AssignAnnotation {
    pub formals: Formals,
    /// Formal holding the bound name.
    pub name: &'static str,
    /// Formal holding the bound value.
    pub value: &'static str,
    /// Formals that select where the binding lands. `delayedAssign()` takes two
    /// environments and only `assign.env` is one of these.
    pub target_env: Formals,
}

impl AssignHandler for AssignAnnotation {
    fn resolve(&self, site: EffectSite, ctx: &mut CallContext<'_>) -> Option<Vec<AssignBinding>> {
        let EffectSite::Call(call) = site else {
            return None;
        };
        let bound = BoundArguments::new(call, self.formals)?;

        // An explicit target environment binds outside the current scope, which
        // we don't currently support.
        if self
            .target_env
            .iter()
            .any(|formal| bound.get(formal).is_some())
        {
            return None;
        }

        let name_expr = bound.get(self.name)?;
        let name = ctx.resolve_static_string(name_expr)?;

        Some(vec![AssignBinding {
            name,
            name_expr: RangedAstPtr::new(name_expr),
            value_expr: bound.get(self.value).map(AstPtr::new),
            target: TargetAccess::Write,
        }])
    }
}

/// Handler for a binding operator (`x %<>% f()`, `x %<~% expr`, `x := expr`).
#[derive(Debug, Clone, Copy)]
pub struct BindingOperatorHandler {
    /// Whether the target is also read (compound operators like `%<>%`).
    pub target: TargetAccess,
}

impl AssignHandler for BindingOperatorHandler {
    fn resolve(&self, site: EffectSite, _ctx: &mut CallContext<'_>) -> Option<Vec<AssignBinding>> {
        let EffectSite::Operator(bin) = site else {
            return None;
        };
        let left = bin.left().ok()?;
        let right = bin.right().ok()?;

        let name = resolve_quoted_symbol_or_string(&left)?;

        Some(vec![AssignBinding {
            name,
            name_expr: RangedAstPtr::new(&left),
            value_expr: Some(AstPtr::new(&right)),
            target: self.target,
        }])
    }
}

/// Extract a name without evaluating it or looking up identifier bindings.
pub fn resolve_quoted_symbol_or_string(value: &AnyRExpression) -> Option<String> {
    match value {
        AnyRExpression::RIdentifier(ident) => Some(ident.name_text()),
        AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
        _ => None,
    }
}
