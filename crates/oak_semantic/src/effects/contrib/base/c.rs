use aether_syntax::RCall;

use crate::effects::BoundArguments;
use crate::effects::CallContext;
use crate::effects::Formals;
use crate::effects::StaticValue;
use crate::effects::ValueHandler;

/// Evaluate `c()` only when every element is a known character vector or
/// `NULL`. R coerces mixed types, but unsupported coercions such as
/// `c("a", 1)` remain unknown rather than producing a guessed value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CHandler;

impl ValueHandler for CHandler {
    fn evaluate(&self, call: &RCall, ctx: &mut CallContext<'_>) -> Option<StaticValue> {
        let formals: Formals = &["...", "recursive", "use.names"];
        let bound = BoundArguments::new(call, formals)?;

        // Distinguish `c()` (which returns `NULL`) from a character vector.
        let mut out: Option<Vec<String>> = None;

        for (formal, value) in bound.arguments() {
            // An empty argument such as `c("a", )` or `c("a", recursive = )`
            // is an error in R.
            let value = value?;

            match formal {
                // Neither option changes character elements, but the call
                // remains unknown unless each supplied option resolves to a boolean.
                // This avoids guessing how R handles values such as `NA`.
                Some("recursive" | "use.names") => {
                    ctx.resolve_static_bool(value)?;
                },
                _ => match ctx.resolve_static_value(value)? {
                    StaticValue::Null => {},
                    StaticValue::Character(elements) => {
                        out.get_or_insert_default().extend(elements)
                    },
                },
            }
        }

        Some(match out {
            Some(elements) => StaticValue::Character(elements),
            None => StaticValue::Null,
        })
    }
}
