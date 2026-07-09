use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
use aether_syntax::RArgument;
use aether_syntax::RCall;
use biome_rowan::AstSeparatedList;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;

/// Effects of a call, resolved against the call site.
#[derive(Debug, Clone, Default)]
pub struct Effects {
    pub arguments: Option<ResolvedArgumentEffects>,
    pub attach: Option<String>,
}

/// The handlers that compute a function's effects.
#[derive(Debug, Clone, Copy)]
pub struct EffectsHandlers {
    pub arguments: Option<&'static dyn EffectHandler<Output = ResolvedArgumentEffects>>,
    pub attach: Option<&'static dyn EffectHandler<Output = String>>,
}

/// Resolver for an effect of a call.
///
/// The single interface behind every effect kind (NSE, attach, and `source`
/// later).
///
/// Handlers are contributed statically for now (a `&'static dyn` in the
/// registry), so the trait is `Sync`, which every registry `static` needs.
pub trait EffectHandler: std::fmt::Debug + Sync {
    type Output;

    /// Resolve this effect for `call`, or `None` when the call isn't in a shape
    /// this handler recognizes.
    ///
    /// `ctx` resolves information the call's own syntax doesn't carry, e.g. what
    /// a `character.only = TRUE` variable is bound to. Unused until that lands.
    fn resolve(&self, call: &RCall, ctx: &CallContext) -> Option<Self::Output>;
}

/// Context for effect handlers.
///
/// Allows querying the properties or static values of arguments.
pub struct CallContext<'a> {
    /// Resolve an identifier to a statically known string value, e.g. a
    /// `character.only` package name for `library()`.
    ///
    /// TODO: This method is currently illustrative. It shows why the methods
    /// are type-erased (so they give access to `self` which can do static
    /// resolution).
    resolve_string: &'a dyn Fn(&str) -> Option<String>,
}

impl<'a> CallContext<'a> {
    pub fn new(resolve_string: &'a dyn Fn(&str) -> Option<String>) -> Self {
        Self { resolve_string }
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

    /// Resolve an argument `name` to a statically known string value, or `None`.
    /// E.g. could be used to implement `character.only` in the `library()` handler.
    pub fn resolve_string(&self, name: &str) -> Option<String> {
        (self.resolve_string)(name)
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

/// A call's resolved NSE arguments: for each argument in call order, the scoped
/// argument it matched, or `None` for a plain argument.
pub type ResolvedArgumentEffects = Vec<Option<&'static Argument>>;

/// Declares how an NSE function's arguments create scopes, and serves as the
/// default [`EffectHandler`] for it by matching the declaration to a call.
#[derive(Debug, Clone, Copy)]
pub struct ArgumentsAnnotation {
    pub arguments: &'static [Argument],
}

/// A single argument that creates an NSE scope.
#[derive(Debug)]
pub struct Argument {
    pub name: &'static str,
    pub position: usize,
    pub scope: NseScope,
    pub timing: NseTiming,
}

impl EffectHandler for ArgumentsAnnotation {
    type Output = ResolvedArgumentEffects;

    fn resolve(&self, call: &RCall, ctx: &CallContext) -> Option<ResolvedArgumentEffects> {
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
                .map(|formal| formal.map(|i| &arguments[i]))
                .collect(),
        )
    }
}

/// Declares how an attach function (`library()`, `require()`) names its package,
/// and serves as the default [`EffectHandler`] for it by extracting that package
/// from a call.
#[derive(Debug, Clone, Copy)]
pub struct AttachAnnotation {
    /// Whether the callee has a `character.only`-style flag. Unread today.
    pub character_only: bool,
}

impl EffectHandler for AttachAnnotation {
    type Output = String;

    fn resolve(&self, call: &RCall, ctx: &CallContext) -> Option<String> {
        // `library()`/`require()` name their package in the `package` formal,
        // the first positional argument.
        let formals = [Formal {
            name: "package",
            position: 0,
        }];
        let matched = ctx.match_arguments(call, &formals);

        let arg_index = matched.iter().position(|formal| *formal == Some(0))?;
        let arg = call.arguments().ok()?.items().iter().nth(arg_index)?.ok()?;
        let value = arg.value()?;

        match &value {
            AnyRExpression::RIdentifier(ident) => Some(ident.name_text()),
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => s.string_text(),
            _ => None,
        }
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
