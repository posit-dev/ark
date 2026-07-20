use crate::effects::contrib::assign_op;
use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::semantic_index::EvalEnv::Current;
use crate::semantic_index::EvalTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    assign_op!("rlang", "%<~%"),
    nse!("rlang", "on_load", ("expr", 0, Current, Lazy)),
];
