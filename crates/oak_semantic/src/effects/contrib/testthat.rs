use crate::effects::contrib::declared;
use crate::effects::contrib::Entry;
use crate::effects::Declaration;
use crate::semantic_index::NseScope::Nested;
use crate::semantic_index::NseTiming::Eager;

pub(crate) fn entries() -> Vec<Entry> {
    vec![declared(
        "testthat",
        "test_that",
        Declaration::new(&["desc", "code"]).nse(1, Nested, Eager),
    )]
}
