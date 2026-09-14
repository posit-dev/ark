//! Executes each scenario on one database so edits exercise incremental evaluation.
//!
//! Panics propagate because Salsa poisons the database after an unwind.

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

/// Print the scenario before execution because hangs and aborts do not unwind.
pub(super) fn start(scenario: &Scenario) -> World {
    recovery::reset();
    eprintln!("{}", scenario.header());
    eprint!("{}", scenario.render());
    let world = World::materialize(&scenario.initial);
    world.query(&scenario.cold_entry);
    world
}

/// Prints the workspace and history while unwinding from a panic.
///
/// `OAK_FUZZ_TRACE=1` also prints each operation before it runs, which captures
/// hangs and aborts that never reach [`Drop::drop()`].
struct Report<'scenario> {
    scenario: &'scenario Scenario,
    trace: bool,
    /// Printed on panic to identify the failed operation.
    current: Cell<Option<String>>,
    /// Recovery firings already printed while tracing.
    printed_firings: Cell<usize>,
}

impl Report<'_> {
    fn new(scenario: &Scenario) -> Report<'_> {
        // Print the replay key before execution because hangs and aborts do not unwind.
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
    /// Matches the database source text so offsets use post-edit text.
    spec: WorkspaceSpec,
    /// Indexed by [`FileId`] to match [`WorkspaceSpec::files`].
    files: Vec<File>,
}

impl World {
    /// Do not evaluate semantic queries here. `cold_entry` must be Salsa's first query.
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

        // Avoid reading a `NAMESPACE` beside the URL-only `DESCRIPTION` path.
        // `workspace::as_packages()` classifies this package from its root kind.
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
                    Some(spec.file(id).program.render().text),
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
                self.spec.file_mut(edit.file).program = edit.program.clone();
                let text = edit.program.render().text;
                // Match `oak_scan::inputs::upsert_editor()` so a `didChange` leaves the
                // file revision unchanged.
                self.files[edit.file.0]
                    .set_source_text_override(&mut self.db)
                    .to(Some(text));
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
        let rendered = self.spec.file(id).program.render();
        let position = match site {
            Site::FirstCall => rendered.first_call.unwrap_or(0),
            Site::LastIdentifier => rendered.last_identifier.unwrap_or(0),
            Site::Eof => rendered.text.len(),
        };
        TextSize::from(position as u32)
    }

    /// Root-relative `source()` targets. An empty result means no edge was recognized.
    pub(super) fn source_targets(&self, id: FileId) -> Vec<String> {
        self.file(id)
            .source_targets(&self.db)
            .iter()
            .map(|target| path_name(target.path(&self.db)))
            .collect()
    }

    pub(super) fn attached_packages(&self, id: FileId) -> Vec<String> {
        self.file(id)
            .attached_packages(&self.db)
            .iter()
            .map(|name| name.text(&self.db).to_string())
            .collect()
    }

    pub(super) fn attached_packages_anywhere(&self, id: FileId) -> Vec<String> {
        self.file(id)
            .attached_packages_anywhere(&self.db)
            .iter()
            .map(|name| name.text(&self.db).to_string())
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
