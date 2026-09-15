//! Executes each scenario on one database so edits exercise incremental
//! evaluation.
//!
//! [`Runner::check()`] catches property panics so the check can shrink
//! failures. The database is dropped during unwinding before the panic is
//! caught.

use std::cell::Cell;
use std::path::Path;

use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::fuzz::artifact::Artifact;
use crate::fuzz::panics::catch_quietly;
use crate::fuzz::panics::install;
use crate::fuzz::panics::Guard;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Scenario;
use crate::fuzz::scenario::Site;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::WorkspaceSpec;
use crate::fuzz::spec::LIBRARY_ROOT;
use crate::fuzz::spec::SCRIPT_ROOT;
use crate::recovery;
use crate::test_path::file_path;
#[cfg(test)]
use crate::test_path::path_name;
use crate::DbInputs;
#[cfg(test)]
use crate::DiagnosticKind;
use crate::File;
use crate::FileRevision;
use crate::Name;
use crate::OakDatabase;
use crate::Package;
use crate::Root;
use crate::RootKind;

/// Owns the failure artifact and the panic hook, so no caller can run a
/// scenario without the context a failure needs.
pub struct Runner {
    artifact: Artifact,
    /// `check()` can only recover a panic's message and location while this guard is alive.
    _hook: Guard,
}

impl Runner {
    pub fn open() -> Runner {
        Runner {
            artifact: Artifact::open(),
            _hook: install(),
        }
    }

    /// Runs `scenario`, returning a panic's message and location on failure.
    pub fn check(&self, scenario: &Scenario) -> std::result::Result<(), String> {
        run_scenario(scenario, &self.artifact)
    }

    /// Runs `scenario` and lets a panic propagate, which is how a fuzzing
    /// engine learns of a crash. The artifact is written ahead of each
    /// operation, so an abort still names the operation in flight.
    pub fn execute(&self, scenario: &Scenario) {
        run(scenario, &self.artifact, traced())
    }

    /// Re-runs `scenario` with operation tracing, letting a panic propagate.
    pub fn replay(&self, scenario: &Scenario) {
        run(scenario, &self.artifact, true)
    }

    /// Path of the artifact naming the scenario and operation in flight.
    pub fn artifact_path(&self) -> &Path {
        self.artifact.path()
    }

    /// Truncates the artifact after a clean run.
    pub fn clear(&self) {
        self.artifact.clear()
    }
}

/// Preserve the original panic details in case the shrunken failure does not
/// reproduce.
fn run_scenario(scenario: &Scenario, artifact: &Artifact) -> std::result::Result<(), String> {
    catch_quietly(|| run(scenario, artifact, traced()))
        .map_err(|panic| format!("{}\n{panic}", scenario.header()))
}

fn run(scenario: &Scenario, artifact: &Artifact, trace: bool) {
    recovery::reset();
    let report = Report::new(scenario, artifact, trace);

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

fn traced() -> bool {
    std::env::var_os("OAK_FUZZ_TRACE").is_some()
}

/// Print the scenario before execution because hangs and aborts do not unwind.
#[cfg(test)]
pub(crate) fn start(scenario: &Scenario) -> World {
    recovery::reset();
    eprintln!("{}", scenario.header());
    eprint!("{}", scenario.render());
    let world = World::materialize(&scenario.initial);
    world.query(&scenario.cold_entry);
    world
}

/// Record each operation before it runs so hangs and aborts leave an artifact.
/// Tracing also prints this context during replay and `OAK_FUZZ_TRACE=1` runs.
struct Report<'scenario> {
    trace: bool,
    /// Recovery firings already printed while tracing.
    printed_firings: Cell<usize>,
    artifact: &'scenario Artifact,
}

impl Report<'_> {
    fn new<'scenario>(
        scenario: &'scenario Scenario,
        artifact: &'scenario Artifact,
        trace: bool,
    ) -> Report<'scenario> {
        if trace {
            eprintln!("{}", scenario.header());
            eprint!("{}", scenario.render());
        }
        artifact.reset(format!("{}\n{}", scenario.header(), scenario.render()));
        Report {
            trace,
            printed_firings: Cell::new(0),
            artifact,
        }
    }

    fn entering(&self, position: &str, operation: &str) {
        let current = format!("{position}: {operation}");
        if self.trace {
            eprintln!("  {current}");
        }
        self.artifact.entering(&current);
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

pub(crate) struct World {
    db: OakDatabase,
    /// Matches the database source text so offsets use post-edit text.
    spec: WorkspaceSpec,
    /// Indexed by [`FileId`] to match [`WorkspaceSpec::files`].
    files: Vec<File>,
}

impl World {
    /// Do not evaluate semantic queries here. `cold_entry` must be Salsa's first query.
    pub(crate) fn materialize(spec: &WorkspaceSpec) -> Self {
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

    pub(crate) fn apply(&mut self, op: &Op) {
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
    #[cfg(test)]
    pub(crate) fn source_targets(&self, id: FileId) -> Vec<String> {
        self.file(id)
            .source_targets(&self.db)
            .iter()
            .map(|target| path_name(target.path(&self.db)))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn attached_packages(&self, id: FileId) -> Vec<String> {
        self.file(id)
            .attached_packages(&self.db)
            .iter()
            .map(|name| name.text(&self.db).to_string())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn attached_packages_anywhere(&self, id: FileId) -> Vec<String> {
        self.file(id)
            .attached_packages_anywhere(&self.db)
            .iter()
            .map(|name| name.text(&self.db).to_string())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn any_source_cycle(&self) -> bool {
        self.spec.ids().any(|id| self.source_cycle_reported(id))
    }

    #[cfg(test)]
    pub(crate) fn source_cycle_reported(&self, id: FileId) -> bool {
        self.file(id)
            .diagnostics(&self.db)
            .iter()
            .any(|diagnostic| diagnostic.kind() == DiagnosticKind::SourceCycle)
    }
}

#[cfg(test)]
mod tests {
    use std::panic::AssertUnwindSafe;

    use super::*;
    use crate::fuzz::corpus;

    /// The artifact must identify the active operation even without unwinding.
    #[test]
    fn test_artifact_records_scenario_and_failing_operation() {
        let scenario = corpus::case("acyclic_pair_closes_then_reopens");
        let artifact = Artifact::open();
        let operation = scenario.ops[0].render();

        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let report = Report::new(&scenario, &artifact, false);
            report.entering("op 0", &operation);
            panic!("simulated hang point");
        }));
        assert!(outcome.is_err());

        let content = std::fs::read_to_string(artifact.path()).unwrap();
        let expected = format!(
            "{}\n{}  current: op 0: {operation}\n",
            scenario.header(),
            scenario.render()
        );
        assert_eq!(content, expected);
    }

    /// A fuzzing engine detects a crash only if the panic reaches it, so
    /// `execute()` must not swallow what `check()` deliberately catches.
    #[test]
    fn test_execute_lets_a_panic_propagate() {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        scenario.cold_entry = Query::Diagnostics(FileId(9));

        let runner = Runner::open();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| runner.execute(&scenario)));

        assert!(outcome.is_err());
    }

    /// `Runner::open()` installs the panic hook itself, so `check()` must recover
    /// the real panic message without a separately installed hook.
    #[test]
    fn test_check_recovers_panic_without_separately_installed_hook() {
        let mut scenario = corpus::case("acyclic_pair_closes_then_reopens");
        scenario.cold_entry = Query::Diagnostics(FileId(9));

        let runner = Runner::open();
        let outcome = runner.check(&scenario);

        assert!(outcome.is_err());
        let message = outcome.unwrap_err();
        assert!(message.contains("index out of bounds"));
        assert!(!message.contains("panicked without reaching the panic hook"));
    }
}
