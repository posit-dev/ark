use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::semantic_index::NseScope::Nested;
use crate::semantic_index::NseTiming::Eager;

pub(crate) static ENTRIES: &[Entry] = &[nse!("testthat", "test_that", ("code", 1, Nested, Eager))];
