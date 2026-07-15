use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
use aether_syntax::RArgument;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use biome_rowan::AstPtr;
use biome_rowan::AstSeparatedList;
// Re-exported so consumers building an `AssignBinding` (custom `Handler`s)
// can name the `name_expr` field's type without depending on oak_core directly.
pub use oak_core::range::RangedAstPtr;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;

/// The owned declaration model and its arg-centric resolver.
pub mod declaration;

/// Per-package tables of which functions carry effects. Private data behind the
/// `lookup`/`annotates` query API below.
mod contrib;

pub use declaration::ArgumentEffect;
pub use declaration::ArgumentRef;
pub use declaration::DeclExpr;
pub use declaration::Declaration;
pub use declaration::EnvironmentEffect;
pub use declaration::EvalMode;
pub use declaration::FormalDef;
pub use declaration::RExpr;
pub use declaration::StaticValue;

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

/// Where a call's effects come from: data-only declarations or custom code.
///
/// The declarative bulk and the imperative escape hatches want different
/// representations, so they get different variants. A [`Declaration`] is plain
/// data ("`x` is `Quote`"), so it borrows straight from the `LazyLock`-parsed
/// registry. A [`Handler`] is code for the shapes a declaration can't express
/// (bquote's `.()` holes, binding operators, assign), and it's `&'static dyn` so
/// a contrib file can add one by referencing its own struct, touching no central
/// enum.
///
/// `Copy`, so [`lookup`] hands it back by value and no borrow into the registry
/// escapes into the walk.
#[derive(Debug, Clone, Copy)]
pub enum EffectSource {
    Declared(&'static Declaration),
    Custom(&'static dyn Handler),
}

/// Look up the effect source of a `(package, function)` pair.
pub fn lookup(package: &str, function: &str) -> Option<EffectSource> {
    contrib::REGISTRY
        .iter()
        .find(|entry| entry.package == package && entry.function == function)
        .map(contrib::Entry::source)
}

/// Whether any registry entry annotates `name`. This is the bare-callee front
/// gate: an unannotated name can't resolve to an effect no matter which provider
/// wins, so recognition skips resolution entirely.
pub fn annotates(name: &str) -> bool {
    contrib::REGISTRY.iter().any(|entry| entry.function == name)
}

/// Where an effect is invoked. Most effects are only ever calls but an Assign
/// effect can also be a binding operator (`x %<>% f`). [`Handler`] takes this to
/// disambiguate rather than a bare call.
pub enum EffectSite<'a> {
    Call(&'a RCall),
    Operator(&'a RBinaryExpression),
}

/// Resolver for shapes a [`Declaration`] can't express.
///
/// One object-safe method, so every custom handler sits behind a single
/// `&'static dyn Handler` in the registry alongside everyone else's. `Sync`
/// because those registry entries are shared across threads.
///
/// A handler fills only its own axis of [`Effects`] (which is `Default`):
/// bquote fills `arguments`, `library` fills `attach`, `source` fills `source`,
/// assign fills `assign`. Call-shaped handlers destructure [`EffectSite::Call`]
/// and return `None` for an operator site; the binding-operator handler does the
/// reverse.
pub trait Handler: std::fmt::Debug + Sync {
    /// Resolve this handler's effect for `site`, or `None` when `site` isn't a
    /// shape it recognizes.
    ///
    /// `ctx` provides semantic resolution, e.g. resolve an argument to a
    /// statically known string or boolean.
    fn resolve(&self, site: EffectSite, ctx: &CallContext) -> Option<Effects>;
}

/// Context for effect handlers.
///
/// Allows querying the properties or static values of arguments. Stateless
/// today, an extension point for information a call's syntax doesn't carry (e.g.
/// resolving a `character.only` variable to its string value) once that lands.
#[derive(Default)]
pub struct CallContext;

impl CallContext {
    pub fn new() -> Self {
        Self
    }

    /// Match `call`'s arguments to `formals`, returning for each call argument
    /// in order the index into `formals` it bound to. Named arguments match
    /// first, then the rest fill by position.
    ///
    /// A stopgap: without the callee's full formal list, a positional argument
    /// only binds a formal declared at that exact position.
    pub fn match_arguments(&self, call: &RCall, formals: &[Formal<'_>]) -> Vec<Option<usize>> {
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
pub struct Formal<'a> {
    pub name: &'a str,
    pub position: usize,
}

/// A call's resolved argument effects: for each argument in call order, the
/// effect it resolved to, or `None` for a plain (standard-eval) argument.
pub type ResolvedArgumentEffects = Vec<Option<ResolvedArgumentEffect>>;

/// The resolved, per-call effect of one argument. The builder consumes these.
#[derive(Debug, Clone)]
pub enum ResolvedArgumentEffect {
    /// Quote plus Eval in a controlled scope, fused.
    Nse { scope: NseScope, timing: NseTiming },
    /// Captured unevaluated. `holes` are the sub-expressions that escape back to
    /// evaluation (e.g. bquote's `.()` contents), walked normally; everything
    /// else in the argument is inert. Empty for a plain `quote()`.
    Quote { holes: Vec<AnyRExpression> },
}

/// Declares how an assign function (`assign()`, `delayedAssign()`) names the
/// variable it binds, and serves as its [`Handler`] by pulling that name out of
/// a call.
#[derive(Debug, Clone, Copy)]
pub struct AssignAnnotation {
    /// Which positional argument holds the bound name, counting only unnamed
    /// arguments (0 for base `assign`/`delayedAssign`).
    pub position: usize,
}

impl Handler for AssignAnnotation {
    fn resolve(&self, site: EffectSite, ctx: &CallContext) -> Option<Effects> {
        let EffectSite::Call(call) = site else {
            return None;
        };
        let args = call.arguments().ok()?;

        // Matched positionally among unnamed arguments, so a leading named
        // argument doesn't shift the count and a named `x =` isn't recognized.
        // The value is the positional right after the name (base
        // `assign(x, value, ...)`).
        //
        // FIXME: A named `value =` isn't captured yet.
        // TODO(nse): Fold onto `match_arguments()` once it's signature-aware,
        // keeping only the `envir`/`pos` bail and the value-after-name read on
        // top.
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
        Some(Effects {
            assign: Some(vec![AssignBinding {
                name,
                name_expr,
                value_expr,
            }]),
            ..Effects::default()
        })
    }
}

/// Handler for a binding operator (`x %<>% f()`, `x %<~% expr`, `x := expr`).
///
/// The operator captures its LHS unevaluated.
#[derive(Debug, Clone, Copy)]
pub struct BindingOperatorHandler;

impl Handler for BindingOperatorHandler {
    fn resolve(&self, site: EffectSite, ctx: &CallContext) -> Option<Effects> {
        let EffectSite::Operator(bin) = site else {
            return None;
        };
        let left = bin.left().ok()?;
        let right = bin.right().ok()?;

        let name = ctx.resolve_quoted_symbol_or_string(&left)?;

        Some(Effects {
            assign: Some(vec![AssignBinding {
                name,
                name_expr: RangedAstPtr::new(&left),
                value_expr: Some(AstPtr::new(&right)),
            }]),
            ..Effects::default()
        })
    }
}

/// Match a named argument against `formals`. Returns the index of the matched
/// formal.
///
/// Should we do partial argument matching? Or rely on partial matching being linted?
fn match_named(arg: &RArgument, formals: &[Formal<'_>], consumed: &[bool]) -> Option<usize> {
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
fn match_positional(formals: &[Formal<'_>], position: usize, consumed: &[bool]) -> Option<usize> {
    formals
        .iter()
        .enumerate()
        .find(|(i, formal)| !consumed[*i] && formal.position == position)
        .map(|(i, _)| i)
}
