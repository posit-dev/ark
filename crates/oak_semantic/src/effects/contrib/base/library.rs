use aether_syntax::AnyRExpression;
use aether_syntax::RCall;
use biome_rowan::AstSeparatedList;

use crate::effects::CallContext;
use crate::effects::EffectHandler;
use crate::effects::Formal;

/// Handler for `library()` and `require()`. Names the attached package from the
/// first argument, read as quoted (the symbol or string as written, so
/// `library(dplyr)` attaches `dplyr`). `character.only = TRUE` flips that
/// argument to standard eval (a value to resolve, `library(pkg, character.only =
/// TRUE)`), matching R. That flag is specific to these callees, so it lives in
/// this handler rather than the shared attach vocabulary.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LibraryHandler;

impl EffectHandler for LibraryHandler {
    type Output = String;

    fn resolve(&self, call: &RCall, ctx: &CallContext<'_>) -> Option<String> {
        // `character.only` sits at signature position 4 in both callees; in
        // practice it's passed by name.
        let formals = [
            Formal {
                name: "package",
                position: 0,
            },
            Formal {
                name: "character.only",
                position: 4,
            },
        ];
        let matched = ctx.match_arguments(call, &formals);

        let args = call.arguments().ok()?;
        let values: Vec<Option<AnyRExpression>> = args
            .items()
            .iter()
            .map(|item| item.ok().and_then(|arg| arg.value()))
            .collect();

        let package = matched
            .iter()
            .position(|formal| *formal == Some(0))
            .and_then(|i| values.get(i))
            .and_then(|value| value.as_ref())?;

        let character_only = matched
            .iter()
            .position(|formal| *formal == Some(1))
            .and_then(|i| values.get(i))
            .and_then(|value| value.as_ref())
            .and_then(|value| ctx.resolve_static_bool(value))
            .unwrap_or(false);

        if character_only {
            ctx.resolve_static_string(package)
        } else {
            ctx.resolve_quoted_symbol_or_string(package)
        }
    }
}
