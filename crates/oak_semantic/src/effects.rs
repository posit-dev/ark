use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
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

/// The `declare()` directive parser: source text -> `Declaration`.
pub mod declare;

/// Per-package tables of which functions carry effects. Private data behind the
/// `lookup`/`annotates` query API below.
mod contrib;

use declaration::argument_name;
use declaration::argument_values;
use declaration::match_signature;
pub use declaration::ArgumentEffect;
pub use declaration::ArgumentRef;
pub use declaration::DeclExpr;
pub use declaration::Declaration;
pub use declaration::EnvOp;
pub use declaration::EnvironmentEffect;
pub use declaration::EvalMode;
pub use declaration::FormalDef;
pub use declaration::RExpr;
pub use declaration::SourcedPath;
pub use declaration::StaticValue;
pub use declare::parse_declaration;
pub use declare::DeclareDiagnostic;
pub use declare::DeclareDiagnosticKind;
pub use declare::ParsedDeclaration;

/// Effects of a call, resolved against the call site.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    /// Per-argument evaluation effects, resolved against the call and aligned
    /// 1:1 with its arguments. `None` at a slot means a plain (standard-eval)
    /// argument.
    pub arguments: Option<ResolvedArgumentEffects>,
    /// Attach a package
    pub attach: Option<String>,
    /// Source one or more files, each with the scope its top-level names land
    /// in. A vector so a collation-style callee can name several; base `source`
    /// resolves to one.
    pub source: Option<Vec<SourcedPath>>,
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
    contrib::lookup(package, function)
}

/// Whether any registry entry annotates `name`. This is the bare-callee front
/// gate: an unannotated name can't resolve to an effect no matter which provider
/// wins, so recognition skips resolution entirely.
pub fn annotates(name: &str) -> bool {
    contrib::annotates(name)
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
///
/// Assign stays custom because it selects its target environment two ways
/// (`envir` and `pos`, the search-path index), which the declaration grammar
/// doesn't model. The name and value reads themselves go through
/// [`match_signature`], so a named `x =` or `value =` binds by name like any
/// other argument.
///
/// [`match_signature`]: crate::effects::declaration::match_signature
#[derive(Debug, Clone, Copy)]
pub struct AssignAnnotation {
    /// The callee's formals, in signature order. Formal 0 (`x`) holds the bound
    /// name and formal 1 (`value`) its value expression; the rest position the
    /// remaining arguments so `match_signature` reads the first two correctly.
    pub formals: &'static [&'static str],
}

impl Handler for AssignAnnotation {
    fn resolve(&self, site: EffectSite, ctx: &CallContext) -> Option<Effects> {
        let EffectSite::Call(call) = site else {
            return None;
        };
        let args = call.arguments().ok()?;

        // An explicit target environment means the binding lands somewhere other
        // than the current scope, so it isn't a fact we can record here. In the
        // future, we could statically recognise some environment selectors like
        // `parent.frame()`. Keyed on the argument name so it fires before
        // matching, and only for `envir`/`pos` (not `delayedAssign`'s
        // `assign.env`, which stays out of scope).
        for item in args.items().iter() {
            let Ok(arg) = item else { continue };
            if let Some(name) = argument_name(&arg) {
                if matches!(name.as_str(), "envir" | "pos") {
                    return None;
                }
            }
        }

        let formals: Vec<FormalDef> = self
            .formals
            .iter()
            .map(|name| FormalDef {
                name: name.to_string(),
                default: None,
            })
            .collect();
        let matched = match_signature(call, &formals);
        let values = argument_values(call);

        // The bound name is formal 0 (`x`). It must resolve to a static string,
        // so a dynamic target (`assign(nm, ...)`) records nothing.
        let name_value = bound_value(&matched, &values, 0)?;
        let name = ctx.resolve_static_string(name_value)?;
        let name_expr = RangedAstPtr::new(name_value);

        // The value is formal 1 (`value`), captured wherever it lands.
        let value_expr = bound_value(&matched, &values, 1).map(AstPtr::new);

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

/// The value bound to the formal at `idx`, per `matched` (from
/// [`match_signature`]) and the call's argument values in the same order.
///
/// [`match_signature`]: crate::effects::declaration::match_signature
fn bound_value<'a>(
    matched: &[Option<usize>],
    values: &'a [Option<AnyRExpression>],
    idx: usize,
) -> Option<&'a AnyRExpression> {
    let pos = matched.iter().position(|bound| *bound == Some(idx))?;
    values.get(pos)?.as_ref()
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

#[cfg(test)]
mod tests {
    use aether_parser::parse;
    use aether_parser::RParserOptions;
    use biome_rowan::AstNode;
    use biome_rowan::WalkEvent;

    use super::*;
    use crate::semantic_index::NseScope;
    use crate::semantic_index::NseTiming;

    /// Parse `source` and return its first call.
    fn first_call(source: &str) -> RCall {
        let parsed = parse(source, RParserOptions::default());
        assert!(!parsed.has_error());
        parsed
            .tree()
            .syntax()
            .preorder()
            .find_map(|event| match event {
                WalkEvent::Enter(node) => RCall::cast(node),
                WalkEvent::Leave(_) => None,
            })
            .unwrap()
    }

    #[test]
    fn lookup_local_is_declared_nested_eager() {
        let Some(EffectSource::Declared(declaration)) = lookup("base", "local") else {
            panic!("expected a declared effect for base::local");
        };
        assert_eq!(declaration.arguments.len(), 1);
        let effect = &declaration.arguments[0];
        assert_eq!(declaration.formals[effect.arg.0].name, "expr");
        assert!(matches!(effect.mode, EvalMode::Nse {
            scope: NseScope::Nested,
            timing: NseTiming::Eager,
        }));
    }

    #[test]
    fn lookup_bquote_is_custom() {
        assert!(matches!(
            lookup("base", "bquote"),
            Some(EffectSource::Custom(_))
        ));
    }

    #[test]
    fn lookup_reactive_is_declared_nested_lazy() {
        let Some(EffectSource::Declared(declaration)) = lookup("shiny", "reactive") else {
            panic!("expected a declared effect for shiny::reactive");
        };
        assert!(matches!(declaration.arguments[0].mode, EvalMode::Nse {
            scope: NseScope::Nested,
            timing: NseTiming::Lazy,
        }));
    }

    /// The bound name for base `assign`, resolved through the handler.
    fn assign_bindings(source: &str) -> Option<Vec<AssignBinding>> {
        let call = first_call(source);
        let handler = AssignAnnotation {
            formals: &["x", "value", "pos", "envir", "inherits", "immediate"],
        };
        handler
            .resolve(EffectSite::Call(&call), &CallContext::new())
            .and_then(|effects| effects.assign)
    }

    #[test]
    fn assign_named_value_then_positional_name_binds_name() {
        // `value =` frees the first positional slot, so `"x"` fills `x`. The old
        // positional-only scan counted `value =` and misread the target.
        let bindings = assign_bindings("assign(value = 1, \"x\")").unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].name, "x");
        assert!(bindings[0].value_expr.is_some());
    }

    #[test]
    fn assign_named_name_binds_name() {
        // A named `x =` is now recognized as the bound name; the positional `1`
        // fills `value`.
        let bindings = assign_bindings("assign(x = \"x\", 1)").unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].name, "x");
        assert!(bindings[0].value_expr.is_some());
    }

    #[test]
    fn assign_named_value_is_captured() {
        // The closed FIXME: a named `value =` is captured wherever it lands.
        let bindings = assign_bindings("assign(\"x\", value = 1)").unwrap();
        assert_eq!(bindings[0].name, "x");
        assert!(bindings[0].value_expr.is_some());
    }

    #[test]
    fn assign_named_envir_bails() {
        assert!(assign_bindings("assign(\"x\", 1, envir = e)").is_none());
    }

    #[test]
    fn assign_named_pos_bails() {
        assert!(assign_bindings("assign(\"x\", 1, pos = 2)").is_none());
    }
}
