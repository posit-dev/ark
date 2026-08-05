use crate::effects::contrib::nse;
use crate::effects::contrib::Entry;
use crate::semantic_index::EvalEnv::Nested;
use crate::semantic_index::EvalTiming::Eager;

pub(crate) static ENTRIES: &[Entry] =
    &[nse!("test_that", ["desc", "code"], ("code", Nested, Eager))];
