//! Executes each scenario on one database so edits exercise incremental
//! evaluation rather than repeated cold evaluation.
//!
//! Panics propagate because Salsa marks the database as poisoned after an
//! unwind.

use std::cell::Cell;

use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::recovery;
use crate::tests::fuzz::scenario::Op;
use crate::tests::fuzz::scenario::Query;
use crate::tests::fuzz::scenario::Scenario;
use crate::tests::fuzz::scenario::Site;
use crate::tests::fuzz::spec::FileId;
use crate::tests::fuzz::spec::Owner;
use crate::tests::fuzz::spec::WorkspaceSpec;
use crate::tests::fuzz::spec::LIBRARY_ROOT;
use crate::tests::fuzz::spec::SCRIPT_ROOT;
use crate::tests::test_db::file_path;
use crate::tests::test_db::path_name;
use crate::DbInputs;
use crate::DiagnosticKind;
use crate::File;
use crate::FileRevision;
use crate::Name;
use crate::OakDatabase;
use crate::Package;
use crate::Root;
use crate::RootKind;

pub(super) fn run(scenario: &Scenario) {
    recovery::reset();
    let report = Report::new(scenario);

    let mut world = World::materialize(&scenario.initial);
    report.entering("cold entry", &scenario.cold_entry.render());
    world.query(&scenario.cold_entry);
    report.firings();

    for (index, op) in scenario.ops.iter().enumerate() {
        report.entering(&format!("op {index}"), &op.render());
        world.apply(op);
        report.firings();
    }
}

/// Report eagerly so explicit scenarios retain context if an operation hangs
/// or aborts before unwinding.
pub(super) fn start(scenario: &Scenario) -> World {
    recovery::reset();
    eprintln!("{}", scenario.header());
    eprint!("{}", scenario.render());
    let world = World::materialize(&scenario.initial);
    world.query(&scenario.cold_entry);
    world
}

/// Prints the concrete workspace and history while unwinding from a panic.
///
/// Eager reporting is restricted to `OAK_FUZZ_TRACE=1` to avoid retaining
/// large captured output. Use it for hangs and aborts, which do not reach
/// `Drop::drop()`.
struct Report<'scenario> {
    scenario: &'scenario Scenario,
    trace: bool,
    /// Operation in flight, named in the dump so the report says which
    /// operation failed rather than only which scenario.
    current: Cell<Option<String>>,
    /// Recovery firings already printed under `trace`.
    printed_firings: Cell<usize>,
}

impl Report<'_> {
    fn new(scenario: &Scenario) -> Report<'_> {
        // Print the replay key first because hangs and aborts do not reach
        // `Drop::drop()`.
        eprintln!("{}", scenario.header());
        let trace = std::env::var_os("OAK_FUZZ_TRACE").is_some();
        if trace {
            eprint!("{}", scenario.render());
        }
        Report {
            scenario,
            trace,
            current: Cell::new(None),
            printed_firings: Cell::new(0),
        }
    }

    fn entering(&self, position: &str, operation: &str) {
        if self.trace {
            eprintln!("  {position}: {operation}");
        }
        self.current.set(Some(format!("{position}: {operation}")));
    }

    fn firings(&self) {
        if !self.trace {
            return;
        }
        let fired = recovery::fired();
        for entry in &fired[self.printed_firings.get()..] {
            eprintln!("      recovered: {entry}");
        }
        self.printed_firings.set(fired.len());
    }
}

impl Drop for Report<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() || self.trace {
            return;
        }
        eprint!("{}", self.scenario.render());
        if let Some(current) = self.current.take() {
            eprintln!("  failed at {current}");
        }
        for entry in recovery::fired() {
            eprintln!("      recovered: {entry}");
        }
    }
}

pub(super) struct World {
    db: OakDatabase,
    /// Kept in step with the database so offset-keyed queries compute their
    /// positions from post-edit text.
    spec: WorkspaceSpec,
    /// Indexed by [`FileId`], matching `spec.files`.
    files: Vec<File>,
}

impl World {
    /// Avoid semantic queries here so `cold_entry` remains the first query Salsa
    /// enters.
    pub(super) fn materialize(spec: &WorkspaceSpec) -> Self {
        let mut db = OakDatabase::new();

        let installed: Vec<Package> = spec
            .installed
            .iter()
            .map(|name| {
                Package::new(
                    &db,
                    file_path(&format!("{LIBRARY_ROOT}/{name}/DESCRIPTION")),
                    name.clone(),
                    FileRevision::zero(),
                    FileRevision::zero(),
                    None,
                    None,
                    Vec::new(),
                    Vec::new(),
                )
            })
            .collect();
        let library = Root::new(
            &db,
            file_path(LIBRARY_ROOT),
            RootKind::Library,
            vec![],
            installed,
        );
        db.library_roots().set_roots(&mut db).to(vec![library]);

        // Use an empty namespace because `Package::namespace()` would otherwise read
        // a `NAMESPACE` beside a URL-only `DESCRIPTION` path. `workspace::as_packages()`
        // classifies the package from its root kind.
        let package = match (&spec.package, spec.package_root()) {
            (Some(name), Some(root)) => Some(Package::new(
                &db,
                file_path(&format!("{root}/DESCRIPTION")),
                name.clone(),
                FileRevision::zero(),
                FileRevision::zero(),
                None,
                Some(Namespace::default()),
                Vec::new(),
                Vec::new(),
            )),
            _ => None,
        };

        let files: Vec<File> = spec
            .ids()
            .map(|id| {
                let owner = match spec.file(id).owner {
                    Owner::Script => None,
                    Owner::Package => package,
                };
                File::new(
                    &db,
                    file_path(&spec.absolute_path(id)),
                    FileRevision::zero(),
                    Some(spec.file(id).contents.clone()),
                    owner,
                )
            })
            .collect();

        let owned = |owner: Owner| -> Vec<File> {
            spec.ids()
                .filter(|&id| spec.file(id).owner == owner)
                .map(|id| files[id.0])
                .collect()
        };

        let mut roots = Vec::new();
        let scripts = owned(Owner::Script);
        if !scripts.is_empty() {
            roots.push(Root::new(
                &db,
                file_path(SCRIPT_ROOT),
                RootKind::Workspace,
                scripts,
                vec![],
            ));
        }
        if let (Some(package), Some(root)) = (package, spec.package_root()) {
            package.set_files(&mut db).to(owned(Owner::Package));
            roots.push(Root::new(
                &db,
                file_path(&root),
                RootKind::Workspace,
                vec![],
                vec![package],
            ));
        }
        db.workspace_roots().set_roots(&mut db).to(roots);

        Self {
            db,
            spec: spec.clone(),
            files,
        }
    }

    pub(super) fn apply(&mut self, op: &Op) {
        match op {
            Op::Query(query) => self.query(query),
            Op::Edit(edit) => {
                self.spec.file_mut(edit.file).contents = edit.contents.clone();
                // Same input the editor path touches on a `didChange`
                // (`oak_scan::inputs::upsert_editor()`), which leaves the file
                // revision alone.
                self.files[edit.file.0]
                    .set_source_text_override(&mut self.db)
                    .to(Some(edit.contents.clone()));
            },
        }
    }

    fn query(&self, query: &Query) {
        let db = &self.db;
        match query {
            Query::Diagnostics(id) => {
                let _ = self.file(*id).diagnostics(db);
            },
            Query::Imports(id) => {
                let _ = self.file(*id).imports(db);
            },
            Query::ImportsAt(id, site) => {
                let _ = self.file(*id).imports_at(db, self.offset(*id, *site));
            },
            Query::ResolveAt(id, site) => {
                let _ = self.file(*id).resolve_at(db, self.offset(*id, *site));
            },
            Query::Resolve(id, name) => {
                let _ = self.file(*id).resolve(db, Name::new(db, name.as_str()));
            },
            Query::UsedPackages(id) => {
                let _ = self.file(*id).used_packages(db);
            },
            Query::SourcedBy(id) => {
                let _ = self.file(*id).sourced_by(db);
            },
            Query::AllPackageDependencies => {
                let _ = crate::workspace::all_package_dependencies(db);
            },
            Query::AllWorkspaceFileDependencies => {
                let _ = crate::workspace::all_workspace_file_dependencies(db);
            },
            Query::AllWorkspaceLoaderDependencies => {
                let _ = crate::workspace::all_workspace_loader_dependencies(db);
            },
            Query::AllWorkspacePackageDependencies => {
                let _ = crate::workspace::all_workspace_package_dependencies(db);
            },
            Query::DefaultSearchPathPackages => {
                let _ = crate::workspace::default_search_path_packages(db);
            },
            Query::SemanticIndex(id) => {
                let _ = self.file(*id).semantic_index(db);
            },
            Query::Exports(id) => {
                let _ = self.file(*id).exports(db);
            },
            Query::AttachedPackages(id) => {
                let _ = self.file(*id).attached_packages(db);
            },
            Query::AttachedPackagesAnywhere(id) => {
                let _ = self.file(*id).attached_packages_anywhere(db);
            },
            Query::InheritedLayers(id, view) => {
                let _ = self.file(*id).inherited_layers(db, *view);
            },
            Query::CrossFileLayers(id, view) => {
                let _ = self.file(*id).cross_file_layers(db, *view);
            },
        }
    }

    fn file(&self, id: FileId) -> File {
        self.files[id.0]
    }

    fn offset(&self, id: FileId, site: Site) -> TextSize {
        let text = &self.spec.file(id).contents;
        let position = match site {
            Site::FirstCall => first_call(text),
            Site::LastIdentifier => last_identifier(text),
            Site::Eof => text.len(),
        };
        TextSize::from(position as u32)
    }

    /// Root-relative paths the file's `source()` calls resolved to. Empty
    /// means no edge was recognized, which an assertion about a cycle has to
    /// rule out.
    pub(super) fn source_targets(&self, id: FileId) -> Vec<String> {
        self.file(id)
            .source_targets(&self.db)
            .iter()
            .map(|target| path_name(target.path(&self.db)))
            .collect()
    }

    pub(super) fn any_source_cycle(&self) -> bool {
        self.spec.ids().any(|id| self.source_cycle_reported(id))
    }

    pub(super) fn source_cycle_reported(&self, id: FileId) -> bool {
        self.file(id)
            .diagnostics(&self.db)
            .iter()
            .any(|diagnostic| diagnostic.kind() == DiagnosticKind::SourceCycle)
    }
}

/// Start of the first `source` or `library` callee, an eager call site.
/// Generated text is ASCII, so byte offsets are char boundaries.
fn first_call(text: &str) -> usize {
    ["source(", "library("]
        .iter()
        .filter_map(|needle| text.find(needle))
        .min()
        .unwrap_or(0)
}

fn last_identifier(text: &str) -> usize {
    let bytes = text.as_bytes();
    let is_identifier_continue =
        |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.';
    let mut cursor = bytes.len();

    while let Some(end) = bytes[..cursor]
        .iter()
        .rposition(|&byte| is_identifier_continue(byte))
    {
        let mut start = end;
        while start > 0 && is_identifier_continue(bytes[start - 1]) {
            start -= 1;
        }

        let starts_with_dot = bytes[start] == b'.';
        let dot_followed_by_digit =
            starts_with_dot && bytes.get(start + 1).is_some_and(u8::is_ascii_digit);
        if bytes[start].is_ascii_alphabetic() || starts_with_dot && !dot_followed_by_digit {
            return start;
        }

        cursor = start;
    }

    0
}

#[test]
fn test_last_identifier_skips_trailing_number() {
    let text = "1\nval_0 <- 2";
    assert_eq!(last_identifier(text), 2);
}

#[test]
fn test_last_identifier_finds_deferred_use() {
    let text = "val_0 <- 1\nread <- function() val_0\n";
    assert_eq!(last_identifier(text), 30);
}
