use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::semantic_index::EvalEnv::Nested;
use crate::semantic_index::EvalTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    nse!("observe", ("x", Nested, Lazy)),
    nse!("reactive", ("x", Nested, Lazy)),
    nse!("renderPlot", ("expr", Nested, Lazy)),
    nse!("renderPrint", ("expr", Nested, Lazy)),
    nse!("renderTable", ("expr", Nested, Lazy)),
    nse!("renderText", ("expr", Nested, Lazy)),
    nse!("renderUI", ("expr", Nested, Lazy)),
];
