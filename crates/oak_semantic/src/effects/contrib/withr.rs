use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::semantic_index::EvalEnv::Current;
use crate::semantic_index::EvalTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    // `defer(expr, envir = parent.frame())` runs `expr` in the caller frame when
    // it exits, the same `Current + Lazy` shape as `on.exit`. A non-default
    // `envir` isn't modeled.
    nse!("defer", ("expr", 0, Current, Lazy)),
];
