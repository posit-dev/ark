use crate::effects::contrib::source;
use crate::effects::contrib::Entry;

pub(super) static ENTRIES: &[Entry] = &[
    // `tar_source(files = "R")` runs every R script under `files`, which is how
    // a `_targets.R` pipeline sees its helper functions. Each element of
    // `files` may be a script or a directory, and the bare `tar_source()` that
    // most pipelines write relies on the default.
    //
    // `files` is a character vector, so `tar_source(c("R", "utils"))` names
    // several paths. Only a single literal is read today.
    source!("tar_source", 0, FileOrDir, Some("R")),
];
