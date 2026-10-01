use aether_syntax::RCall;

use crate::effects::CallContext;

/// Values recognized by static evaluation of effect arguments. Only character
/// vectors without missing values and `NULL` are represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticValue {
    Null,
    Character(Vec<String>),
}

impl StaticValue {
    /// Do not treat `NULL` as an empty character vector. `source()` and
    /// `assign()` reject it as an argument.
    pub fn into_character(self) -> Option<Vec<String>> {
        match self {
            StaticValue::Character(elements) => Some(elements),
            StaticValue::Null => None,
        }
    }
}

/// Consulted only when an effect argument needs a pure call's static value.
/// Implementations must be `Sync` because the registry shares them through
/// statics.
pub trait ValueHandler: std::fmt::Debug + Sync {
    /// Evaluate `call`, or `None` when its value is not statically known.
    fn evaluate(&self, call: &RCall, ctx: &mut CallContext<'_>) -> Option<StaticValue>;
}
