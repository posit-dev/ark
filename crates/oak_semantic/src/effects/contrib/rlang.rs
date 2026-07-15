use crate::effects::contrib::custom;
use crate::effects::contrib::declared;
use crate::effects::contrib::Entry;
use crate::effects::BindingOperatorHandler;
use crate::effects::Declaration;
use crate::semantic_index::NseScope::Current;
use crate::semantic_index::NseTiming::Lazy;

pub(crate) fn entries() -> Vec<Entry> {
    vec![
        custom("rlang", "%<~%", &BindingOperatorHandler),
        declared(
            "rlang",
            "on_load",
            Declaration::new(&["expr"]).nse(0, Current, Lazy),
        ),
    ]
}
