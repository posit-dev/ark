use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::semantic_index::EvalEnv::Nested;
use crate::semantic_index::EvalTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    nse!("observe", ("x", 0, Nested, Lazy)),
    nse!("reactive", ("x", 0, Nested, Lazy)),
    nse!("renderPlot", ("expr", 0, Nested, Lazy)),
    nse!("renderPrint", ("expr", 0, Nested, Lazy)),
    nse!("renderTable", ("expr", 0, Nested, Lazy)),
    nse!("renderText", ("expr", 0, Nested, Lazy)),
    nse!("renderUI", ("expr", 0, Nested, Lazy)),
];
