use aether_syntax::AnyRExpression;
use aether_syntax::AnyRValue;
use aether_syntax::RCall;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use oak_core::syntax_ext::RIdentifierExt;
use oak_core::syntax_ext::RStringValueExt;

use crate::effects::CallContext;
use crate::effects::CalleeOrigin;
use crate::effects::CalleeResolution;
use crate::effects::FunctionHandlers;
use crate::effects::StaticValue;
use crate::semantic_index::AmbiguityReason;

/// Consulted only when an effect argument needs a pure call's static value.
/// Implementations must be `Sync` because the registry shares them through
/// statics.
pub trait ValueHandler: std::fmt::Debug + Sync {
    /// Evaluate `call`, or `None` when its value is not statically known.
    fn evaluate(&self, call: &RCall, ctx: &mut CallContext<'_>) -> Option<StaticValue>;
}

/// A callee that static evaluation resolved through the imports resolver
/// because no local binding was visible at the call site. The resolver's
/// answer depends on the imports the file sees, which differ between sourcing
/// contexts, so the evaluated value holds only under the imports used here. A
/// failed lookup counts too: in another context the name could resolve to a
/// value handler.
///
/// Local callees are decided by the file itself and qualified callees name
/// their package, so neither depends on imports. Dynamic callees are never
/// looked up.
#[derive(Debug, Clone)]
pub struct CalleeImport {
    pub name: String,
    pub call_range: TextRange,
    /// Flow uncertainty relevant to the callee's value handler, for the
    /// consumer to report.
    pub ambiguity: Option<AmbiguityReason>,
}

impl CallContext<'_> {
    /// Recognize string literals, `NULL`, and calls with a registered value
    /// handler. Other expressions are not evaluated or coerced.
    pub fn resolve_static_value(&mut self, value: &AnyRExpression) -> Option<StaticValue> {
        match value {
            AnyRExpression::AnyRValue(AnyRValue::RStringValue(s)) => {
                Some(StaticValue::Character(vec![s.string_text()?]))
            },
            AnyRExpression::RNullExpression(_) => Some(StaticValue::Null),
            AnyRExpression::RCall(call) => {
                let resolution = self.scope.resolve_callee(call);
                self.record_callee_import(call, &resolution);
                resolution.handlers?.value?.evaluate(call, self)
            },
            _ => None,
        }
    }

    /// Require a character vector rather than treating `NULL` as an empty one.
    pub fn resolve_static_character(&mut self, value: &AnyRExpression) -> Option<Vec<String>> {
        self.resolve_static_value(value)?.into_character()
    }

    /// Require exactly one character element for a scalar argument.
    pub fn resolve_static_string(&mut self, value: &AnyRExpression) -> Option<String> {
        let mut elements = self.resolve_static_character(value)?;
        if elements.len() != 1 {
            return None;
        }
        elements.pop()
    }

    /// Recognize the literals `TRUE` and `FALSE`.
    pub fn resolve_static_bool(&self, value: &AnyRExpression) -> Option<bool> {
        match value {
            AnyRExpression::RTrueExpression(_) => Some(true),
            AnyRExpression::RFalseExpression(_) => Some(false),
            _ => None,
        }
    }

    /// Record `call` if its callee was resolved through imports.
    ///
    /// Keep only the first ambiguity record per call site. Multiple handlers
    /// can evaluate the same argument, repeating nested-call lookups and
    /// otherwise causing duplicate diagnostics. Repeated lookups return the
    /// same resolution because bindings and attaches do not change while
    /// handlers for one call run.
    fn record_callee_import(&mut self, call: &RCall, resolution: &CalleeResolution) {
        if resolution.origin != CalleeOrigin::Import {
            return;
        }
        let Ok(AnyRExpression::RIdentifier(ident)) = call.function() else {
            return;
        };
        let call_range = call.syntax().text_trimmed_range();
        if self
            .callee_imports
            .iter()
            .any(|import| import.call_range == call_range)
        {
            return;
        }
        self.callee_imports.push(CalleeImport {
            name: ident.name_text(),
            call_range,
            ambiguity: resolution.ambiguity(FunctionHandlers::has_value),
        });
    }
}
