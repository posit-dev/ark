//! Materializes a scenario database and executes its queries and edits.

use biome_rowan::TextSize;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::file_reader::EmptyFileReader;
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
use crate::Root;
use crate::RootKind;

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
        let mut db = OakDatabase::with_file_reader(EmptyFileReader);

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
            let package_files: Vec<File> = spec
                .ids()
                .filter(
                    |&id| matches!(spec.file(id).owner, Owner::Package(owner) if owner.0 == index),
                )
                .map(|id| files[id.0])
                .collect();
            package.set_files(&mut db).to(package_files);
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

    pub(super) fn query(&self, query: &Query) {
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
            Query::PackageResolve(id, name, visibility) => {
                let _ = self
                    .package(*id)
                    .resolve(db, Name::new(db, name.as_str()), *visibility);
            },
        }
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
