use crate::effects::contrib::assign_op;
use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::effects::TargetAccess::Write;
use crate::semantic_index::EvalEnv::Current;
use crate::semantic_index::EvalTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    assign_op!("rlang", "%<~%", Write),
    nse!("rlang", "on_load", ("expr", 0, Current, Lazy)),
    // `defer(expr, env = caller_env())` runs `expr` in the caller frame when it
    // exits. Written in a function, that frame is the function itself, so it's
    // `Current + Lazy` like `on.exit`. A non-default `env` isn't modeled.
    nse!("rlang", "defer", ("expr", 0, Current, Lazy)),
];
