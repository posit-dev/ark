use aether_syntax::AnyRArgumentName;
use aether_syntax::RArgument;
use aether_syntax::RCall;
use biome_rowan::AstSeparatedList;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use crate::effects::CallContext;
use crate::effects::EffectHandler;
use crate::effects::StaticValue;

/// Evaluate `c()` only when every element is a known character vector or
/// `NULL`. R coerces mixed types, but unsupported coercions such as
/// `c("a", 1)` remain unknown rather than producing a guessed value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CHandler;

impl EffectHandler for CHandler {
    type Output = StaticValue;

    fn resolve(&self, call: &RCall, ctx: &mut CallContext<'_>) -> Option<StaticValue> {
        // Distinguish `c()` (which returns `NULL`) from a character vector.
        let mut out: Option<Vec<String>> = None;

        for item in call.arguments().ok()?.items().iter() {
            let arg = item.ok()?;

            // `recursive` and `use.names` follow `...` in `c()`'s formals, so
            // only exact names select them. Positional arguments are elements,
            // unlike the formals handled by `bind_arguments()`.
            if matches!(
                argument_name(&arg).as_deref(),
                Some("recursive" | "use.names")
            ) {
                continue;
            }

            // A missing argument such as `c("a", )` is an error in R.
            let value = arg.value()?;

            match ctx.resolve_static_value(&value)? {
                StaticValue::Null => {},
                StaticValue::Character(elements) => out.get_or_insert_default().extend(elements),
            }
        }

        Some(match out {
            Some(elements) => StaticValue::Character(elements),
            None => StaticValue::Null,
        })
    }
}

fn argument_name(arg: &RArgument) -> Option<String> {
    match arg.name_clause()?.name().ok()? {
        AnyRArgumentName::RIdentifier(ident) => Some(ident.name_text()),
        AnyRArgumentName::RStringValue(s) => s.string_text(),
        _ => None,
    }
}
