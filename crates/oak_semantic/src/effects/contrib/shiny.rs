use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::semantic_index::NseScope::Nested;
use crate::semantic_index::NseTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    nse!("shiny", "observe", ("x", 0, Nested, Lazy)),
    nse!("shiny", "reactive", ("x", 0, Nested, Lazy)),
    nse!("shiny", "renderPlot", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderPrint", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderTable", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderText", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderUI", ("expr", 0, Nested, Lazy)),
];
