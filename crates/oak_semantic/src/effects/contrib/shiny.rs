use crate::effects::contrib::declared;
use crate::effects::contrib::Entry;
use crate::effects::Declaration;
use crate::semantic_index::NseScope::Nested;
use crate::semantic_index::NseTiming::Lazy;

pub(crate) fn entries() -> Vec<Entry> {
    vec![
        declared(
            "shiny",
            "observe",
            Declaration::new(&["x"]).nse(0, Nested, Lazy),
        ),
        declared(
            "shiny",
            "reactive",
            Declaration::new(&["x"]).nse(0, Nested, Lazy),
        ),
        declared(
            "shiny",
            "renderPlot",
            Declaration::new(&["expr"]).nse(0, Nested, Lazy),
        ),
        declared(
            "shiny",
            "renderPrint",
            Declaration::new(&["expr"]).nse(0, Nested, Lazy),
        ),
        declared(
            "shiny",
            "renderTable",
            Declaration::new(&["expr"]).nse(0, Nested, Lazy),
        ),
        declared(
            "shiny",
            "renderText",
            Declaration::new(&["expr"]).nse(0, Nested, Lazy),
        ),
        declared(
            "shiny",
            "renderUI",
            Declaration::new(&["expr"]).nse(0, Nested, Lazy),
        ),
    ]
}
