use crate::effects::contrib::custom;
use crate::effects::contrib::Entry;
use crate::effects::BindingOperatorHandler;

/// rlang's custom contribution. The binding operator `%<~%` isn't a call, so it
/// stays a handler; `on_load`'s NSE effect lives in `rlang.ty.R`.
pub(crate) fn entries() -> Vec<Entry> {
    vec![custom("rlang", "%<~%", &BindingOperatorHandler)]
}
