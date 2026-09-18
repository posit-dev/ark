//! Materializes a scenario database and executes its queries and edits.

use std::path::Path;

use biome_rowan::TextSize;
use camino::Utf8PathBuf;
use oak_package_metadata::namespace::Namespace;
use rustc_hash::FxHashMap;
use salsa::Setter;

use super::Observe;
use super::Observed;
use crate::classify_in_package;
use crate::file_reader::MapFileReader;
use crate::fuzz::scenario::Op;
use crate::fuzz::scenario::Query;
use crate::fuzz::scenario::Site;
use crate::fuzz::spec::FileId;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::PackageId;
use crate::fuzz::spec::PackageKind;
use crate::fuzz::spec::WorkspaceSpec;
use crate::fuzz::spec::LIBRARY_ROOT;
use crate::fuzz::spec::SCRIPT_ROOT;
use crate::test_path::file_path;
#[cfg(test)]
use crate::test_path::path_name;
use crate::DbInputs;
#[cfg(test)]
use crate::DiagnosticKind;
use crate::File;
use crate::FileRevision;
use crate::Name;
#[cfg(test)]
use crate::NamespaceVisibility;
use crate::OakDatabase;
use crate::Package;
use crate::PackagePlacement;
use crate::Root;
use crate::RootKind;
use crate::SourceDb;

pub(crate) struct World {
    db: OakDatabase,
    /// Matches the database source text so offsets use post-edit text.
    spec: WorkspaceSpec,
    /// Indexed by [`FileId`] to match [`WorkspaceSpec::files`].
    files: Vec<File>,
    /// Indexed by [`PackageId`] to match [`WorkspaceSpec::packages`].
    packages: Vec<Package>,
}

impl World {
    /// Do not evaluate semantic queries here. `cold_entry` must be Salsa's first query.
    pub(crate) fn materialize(spec: &WorkspaceSpec) -> Self {
        // `DESCRIPTION` lacks NAMESPACE's override path, so serve it through
        // the reader to run `Package` metadata queries through the production parser.
        let description_texts: FxHashMap<Utf8PathBuf, String> = spec
            .packages
            .iter()
            .filter(|package_spec| package_spec.kind == PackageKind::Workspace)
            .filter_map(|package_spec| {
                let path = file_path(&format!("{}/DESCRIPTION", package_spec.directory()));
                let path = path.as_path()?.to_path_buf();
                Some((path, package_spec.description_text()))
            })
            .collect();
        let mut db = OakDatabase::with_file_reader(MapFileReader::new(description_texts));

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

        // Parse the modeled directives with the production parser and supply an
        // override to avoid disk reads at synthetic paths. Validation has already
        // checked this text, so a parse failure here is a harness error.
        let packages: Vec<Package> = spec
            .packages
            .iter()
            .map(|package_spec| {
                let namespace = match Namespace::parse(&package_spec.namespace_text()) {
                    Ok(namespace) => namespace,
                    Err(err) => panic!(
                        "harness bug: generated NAMESPACE for {:?} failed to parse: {err:?}",
                        package_spec.name
                    ),
                };
                Package::new(
                    &db,
                    file_path(&format!("{}/DESCRIPTION", package_spec.directory())),
                    package_spec.name.clone(),
                    FileRevision::zero(),
                    FileRevision::zero(),
                    None,
                    Some(namespace),
                    Vec::new(),
                    Vec::new(),
                )
            })
            .collect();

        let mut library_packages = installed;
        library_packages.extend(
            spec.packages
                .iter()
                .zip(&packages)
                .filter(|(package_spec, _)| package_spec.kind == PackageKind::Library)
                .map(|(_, &package)| package),
        );
        let library = Root::new(
            &db,
            file_path(LIBRARY_ROOT),
            RootKind::Library,
            vec![],
            library_packages,
        );
        db.library_roots().set_roots(&mut db).to(vec![library]);

        let files: Vec<File> = spec
            .ids()
            .map(|id| {
                let owner = match spec.file(id).owner {
                    Owner::Script => None,
                    Owner::Package(package_id) => Some(packages[package_id.0]),
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

        let mut roots = Vec::new();
        let scripts: Vec<File> = spec
            .ids()
            .filter(|&id| spec.file(id).owner == Owner::Script)
            .map(|id| files[id.0])
            .collect();
        if !scripts.is_empty() {
            roots.push(Root::new(
                &db,
                file_path(SCRIPT_ROOT),
                RootKind::Workspace,
                scripts,
                vec![],
            ));
        }

        for (index, package_spec) in spec.packages.iter().enumerate() {
            if package_spec.kind != PackageKind::Workspace {
                continue;
            }
            let package = packages[index];
            let owned = spec.ids().filter(
                |&id| matches!(spec.file(id).owner, Owner::Package(owner) if owner.0 == index),
            );

            // Match scanner placement: direct `R/` children are package files,
            // and other package files are standalone scripts.
            let mut candidates: Vec<File> = Vec::new();
            let mut scripts: Vec<File> = Vec::new();
            for id in owned {
                let absolute = spec.absolute_path(id);
                match classify_in_package(
                    Path::new(&package_spec.directory()),
                    Path::new(&absolute),
                ) {
                    PackagePlacement::File => candidates.push(files[id.0]),
                    PackagePlacement::Script => scripts.push(files[id.0]),
                    // `validate()` rejects nested `R/` files before materialization.
                    PackagePlacement::Skip => {
                        panic!("harness bug: {absolute} is nested below `R/` and cannot be loaded")
                    },
                }
            }
            // Partition direct `R/` children through `package.collation()` so
            // `DESCRIPTION` parsing, not a second copy of `PackageSpec::collate`,
            // determines which omitted files become scripts.
            let (collation, leftover) = split_by_collate(&db, candidates, package);
            scripts.extend(leftover);
            package.set_files(&mut db).to(collation);
            package.set_scripts(&mut db).to(scripts);
            roots.push(Root::new(
                &db,
                file_path(&package_spec.directory()),
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
            packages,
        }
    }

    pub(crate) fn apply(&mut self, op: &Op, observer: &mut dyn Observe) -> Observed {
        match op {
            Op::Query(query) => return self.query(query, observer),
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
        Observed::proceed(None)
    }

    /// Calls `observer` only after the resolution query returns so it cannot warm the database first. [`Observed::reference`] identifies recovery firings from the observer's reference execution. See [`Observe`].
    pub(super) fn query(&self, query: &Query, observer: &mut dyn Observe) -> Observed {
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
                let definitions = self.file(*id).resolve_at(db, self.offset(*id, *site));
                return observer.resolved(db, &self.spec, query, &definitions);
            },
            Query::Resolve(id, name) => {
                let definitions = self.file(*id).resolve(db, Name::new(db, name.as_str()));
                return observer.resolved(db, &self.spec, query, &definitions);
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
            Query::PackageResolve(id, name, visibility) => {
                let definitions =
                    self.package(*id)
                        .resolve(db, Name::new(db, name.as_str()), *visibility);
                return observer.resolved(db, &self.spec, query, &definitions);
            },
        }
        Observed::proceed(None)
    }

    fn file(&self, id: FileId) -> File {
        self.files[id.0]
    }

    fn package(&self, id: PackageId) -> Package {
        self.packages[id.0]
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

    /// The lookup layers a file's loader supplies, in priority order, rendered
    /// as `File(basename)` or `Package(name)`. This shows which loader claimed
    /// the file even when a source cycle degrades what it resolves.
    #[cfg(test)]
    pub(crate) fn import_layers(&self, id: FileId) -> Vec<String> {
        use crate::ImportLayer;

        self.file(id)
            .imports(&self.db)
            .iter()
            .map(|layer| match layer {
                ImportLayer::From(package) => format!("From({})", package.name(&self.db)),
                ImportLayer::Package(package) => format!("Package({})", package.name(&self.db)),
                ImportLayer::File(file) | ImportLayer::SourcingFile { file, .. } => {
                    format!("File({})", path_name(file.path(&self.db)))
                },
            })
            .collect()
    }

    /// Returns the load-context loader for `id`, or `None` for standalone files.
    /// Unlike [`Self::import_layers()`], this excludes random `library()` attaches
    /// and `source()` edges.
    #[cfg(test)]
    pub(crate) fn loader_name(&self, id: FileId) -> Option<&'static str> {
        crate::load_context::loader(&self.db, self.file(id)).map(|info| info.name)
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

    #[cfg(test)]
    pub(crate) fn package_description_path(&self, id: PackageId) -> String {
        path_name(self.package(id).description_path(&self.db))
    }

    #[cfg(test)]
    pub(crate) fn package_exports(&self, id: PackageId) -> Vec<String> {
        self.package(id).namespace(&self.db).exports.to_vec()
    }

    /// Preserves the distinction between an absent `Collate:` field and an
    /// explicitly empty one.
    #[cfg(test)]
    pub(crate) fn package_collation(&self, id: PackageId) -> Option<Vec<String>> {
        self.package(id).collation(&self.db).clone()
    }

    #[cfg(test)]
    pub(crate) fn package_imported_from(&self, id: PackageId) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = self
            .package(id)
            .imported_from(&self.db)
            .iter()
            .map(|(name, source)| (name.clone(), source.clone()))
            .collect();
        entries.sort();
        entries
    }

    #[cfg(test)]
    pub(crate) fn package_resolve(
        &self,
        id: PackageId,
        name: &str,
        visibility: NamespaceVisibility,
    ) -> Vec<String> {
        self.package(id)
            .resolve(&self.db, Name::new(&self.db, name), visibility)
            .iter()
            .map(|definition| path_name(definition.file(&self.db).path(&self.db)))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn file_resolve(&self, id: FileId, name: &str) -> Vec<String> {
        self.file(id)
            .resolve(&self.db, Name::new(&self.db, name))
            .iter()
            .map(|definition| path_name(definition.file(&self.db).path(&self.db)))
            .collect()
    }
}

/// Orders direct `R/` children by parsed `Collate:` entries and returns
/// omitted files as standalone scripts. An absent `Collate:` keeps every
/// candidate loadable in draw order.
///
/// Read `package.collation()` to exercise `DESCRIPTION` parsing through
/// `MapFileReader`, rather than duplicating `PackageSpec::collate`.
/// `leftover` preserves input order so identical seeds materialize identically.
fn split_by_collate(
    db: &dyn SourceDb,
    candidates: Vec<File>,
    package: Package,
) -> (Vec<File>, Vec<File>) {
    let Some(order) = package.collation(db).clone() else {
        return (candidates, Vec::new());
    };

    let basename = |file: File| file.path(db).file_name().map(|name| name.to_string());

    let loadable = order
        .iter()
        .filter_map(|name| {
            candidates
                .iter()
                .copied()
                .find(|file| basename(*file).as_deref() == Some(name.as_str()))
        })
        .collect();
    let leftover = candidates
        .into_iter()
        .filter(|file| {
            !order
                .iter()
                .any(|name| basename(*file).as_deref() == Some(name.as_str()))
        })
        .collect();

    (loadable, leftover)
}
