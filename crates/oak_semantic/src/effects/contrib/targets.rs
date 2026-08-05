use crate::effects::contrib::source;
use crate::effects::contrib::Entry;
use crate::effects::DirWalk;
use crate::effects::SourceTarget;

pub(super) static ENTRIES: &[Entry] = &[
    // `tar_source()` loads scripts from `files`, defaulting to `R`. A path may
    // name a script or directory.
    //
    // Directory paths recurse because `file_list_files()` calls
    // `list.files(recursive = TRUE)`.
    //
    // The scanner reads one literal even though `files` is a character vector.
    source!(
        "tar_source",
        ["files"],
        "files",
        SourceTarget::FileOrDir(DirWalk::Recursive),
        Some("R")
    ),
];
