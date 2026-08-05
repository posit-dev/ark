use aether_syntax::RCall;

use crate::effects::CallContext;
use crate::effects::EffectHandler;
use crate::effects::Formals;

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
        let formals: Formals = &["package", "help", "pos", "lib.loc", "character.only"];
        let bound = ctx.bind_arguments(call, formals);

        let package = bound.get("package")?;
        let character_only = bound
            .get("character.only")
            .and_then(|value| ctx.resolve_static_bool(value))
            .unwrap_or(false);

        if character_only {
            ctx.resolve_static_string(package)
        } else {
            ctx.resolve_quoted_symbol_or_string(package)
        }
    }
}
