//! Shiny load-context discovery for `shiny::runApp()`.
//!
//! Detection reads paths, workspace listings, and `source_text()`, not a file's
//! semantic index, so [`File::cross_file_layers()`] can call it while building
//! that index.

use camino::Utf8Path;

use crate::directory::files_in_directory;
use crate::file_imports::CollationView;
use crate::load_context::collation_visible_files;
use crate::load_context::in_r_directory;
use crate::load_context::LoadContext;
use crate::load_context::SearchPathTail;
use crate::Db;
use crate::File;

/// A file `shiny::runApp()` loads through `loadSupport()` or as an entry point.
/// `None` means Shiny does not load it, including `R/` files disabled by
/// `_disable_autoload.R`. Checking `R/` membership first keeps `R/app.R` in its
/// enclosing app's collation rather than treating it as an app root.
pub(crate) fn load_context(db: &dyn Db, file: File, view: CollationView) -> Option<LoadContext> {
    if in_r_directory(file, db) {
        let autoload = shiny_autoload(db, file).as_deref()?;
        return Some(autoload_context(db, file, view, autoload));
    }
    if let Some(autoload) = shiny_autoload(db, file).as_deref() {
        return Some(entry_context(file, autoload));
    }
    // `global.R` runs first in the global environment, so it cannot see app bindings.
    is_shiny_global_file(file, db).then(global_context)
}

/// An `R/` file `loadSupport()` sources: the plain `R/` collation, plus
/// whatever ran before the directory did.
fn autoload_context(
    db: &dyn Db,
    file: File,
    view: CollationView,
    autoload: &[File],
) -> LoadContext {
    let mut visible_files = collation_visible_files(db, file, view);

    // `global.R` loads before `R/` into its parent environment, so every sibling
    // shadows it.
    visible_files.extend(autoload);

    LoadContext {
        visible_files,
        namespace_owner: None,
        implicit_attaches: vec!["shiny"],
        search_path_tail: SearchPathTail::Default,
        fixed_load_order: false,
    }
}

/// A Shiny entry point after `loadSupport()` evaluates `global.R` and adjacent
/// `R/` files. It sees the full support set because it is not a collation member.
fn entry_context(file: File, autoload: &[File]) -> LoadContext {
    LoadContext {
        // `R/` bindings shadow `global.R` through reverse load order and the
        // child environment created by `loadSupport()`.
        visible_files: autoload
            .iter()
            .rev()
            .copied()
            .filter(|support| *support != file)
            .collect(),
        namespace_owner: None,
        implicit_attaches: vec!["shiny"],
        search_path_tail: SearchPathTail::Default,
        fixed_load_order: false,
    }
}

fn global_context() -> LoadContext {
    LoadContext {
        visible_files: Vec::new(),
        namespace_owner: None,
        implicit_attaches: vec!["shiny"],
        search_path_tail: SearchPathTail::Default,
        fixed_load_order: false,
    }
}

/// Shiny support files preceding `file`, in load order. `None` means Shiny does
/// not load `file`, while `Some(vec![])` still records its implicit `shiny`
/// attachment.
///
/// This tracked query filters workspace file listings to the app. The separately
/// tracked [`is_shiny_entry_file()`] lets unchanged entry-point classification
/// backdate callers after a source edit.
#[salsa::tracked(returns(ref))]
pub(crate) fn shiny_autoload(db: &dyn Db, file: File) -> Option<Vec<File>> {
    // `R/` files get only `global.R` here because collation supplies siblings.
    // Disabled autoload removes both their Shiny layers and implicit attachment.
    //
    // Prefer an enclosing app so `R/app.R` remains its module rather than a
    // separate app rooted at `R/`.
    if let Some(app_dir) = enclosing_app_dir(file, db) {
        if r_autoload_disabled(db, app_dir) {
            return None;
        }
        return Some(shiny_global_file(db, app_dir).into_iter().collect());
    }

    // `loadSupport()` attaches `shiny` even when no support files exist.
    let app_dir = shiny_app_dir(file, db)?;
    Some(shiny_support_files(db, app_dir))
}

/// RStudio entry-point filenames and their required marker calls.
/// Text matching intentionally preserves RStudio-compatible comment and string
/// false positives while avoiding a semantic-index read during index construction.
const SHINY_ENTRY_FILES: [(&str, &str); 3] = [
    ("app.R", "shinyApp"),
    ("ui.R", "shinyUI"),
    ("server.R", "shinyServer"),
];

/// Files `loadSupport()` evaluates before app code, in load order.
fn shiny_support_files(db: &dyn Db, app_dir: &Utf8Path) -> Vec<File> {
    let mut files: Vec<File> = shiny_global_file(db, app_dir).into_iter().collect();

    // `_disable_autoload.R` leaves `global.R` in the support set.
    if r_autoload_disabled(db, app_dir) {
        return files;
    }

    files.extend(files_in_directory(db, &app_dir.join("R")));
    files
}

/// Whether `_disable_autoload.R` prevents `loadSupport()` from loading `R/`
/// files. `global.R` remains in the support list.
fn r_autoload_disabled(db: &dyn Db, app_dir: &Utf8Path) -> bool {
    files_in_directory(db, &app_dir.join("R"))
        .iter()
        .any(|file| is_named(*file, db, "_disable_autoload.R"))
}

/// Whether `file` is an app's `global.R`, which `loadSupport()` loads even when
/// `_disable_autoload.R` is present.
fn is_shiny_global_file(file: File, db: &dyn Db) -> bool {
    if !is_named(file, db, "global.R") {
        return false;
    }
    let Some(dir) = file.path(db).as_path().and_then(Utf8Path::parent) else {
        return false;
    };
    is_shiny_dir(db, dir)
}

/// The Shiny app directory whose `R/` holds `file`, if any. `None` unless
/// `file` sits directly in an `R/` that an app encloses.
fn enclosing_app_dir(file: File, db: &dyn Db) -> Option<&Utf8Path> {
    if !in_r_directory(file, db) {
        return None;
    }
    let app_dir = file.path(db).as_path()?.parent()?.parent()?;
    is_shiny_dir(db, app_dir).then_some(app_dir)
}

fn shiny_app_dir(file: File, db: &dyn Db) -> Option<&Utf8Path> {
    if !is_shiny_entry_file(db, file) {
        return None;
    }
    file.path(db).as_path()?.parent()
}

/// Whether `dir` has a Shiny entry point and therefore autoloads its adjacent `R/`.
fn is_shiny_dir(db: &dyn Db, dir: &Utf8Path) -> bool {
    files_in_directory(db, dir)
        .iter()
        .any(|file| is_shiny_entry_file(db, *file))
}

#[salsa::tracked(returns(copy))]
fn is_shiny_entry_file(db: &dyn Db, file: File) -> bool {
    SHINY_ENTRY_FILES
        .iter()
        .any(|(name, marker)| is_named(file, db, name) && file.source_text(db).contains(marker))
}

fn shiny_global_file(db: &dyn Db, app_dir: &Utf8Path) -> Option<File> {
    files_in_directory(db, app_dir)
        .into_iter()
        .find(|file| is_named(*file, db, "global.R"))
}

/// Shiny matches these filenames case-insensitively, unlike the exact `R/`
/// convention in [`in_r_directory()`].
fn is_named(file: File, db: &dyn Db, name: &str) -> bool {
    file.path(db)
        .file_name()
        .is_some_and(|basename| basename.eq_ignore_ascii_case(name))
}
