mod db;
mod definition;
mod diagnostic;
mod directory;
mod file;
mod file_diagnostics;
mod file_exports;
mod file_imports;
mod file_reader;
mod file_resolve;
mod file_revision;
mod file_source_site;
// Non-test fuzz builds use only `Runner::open()`, `execute_json()`, and
// `mutate_json()`, while unit tests exercise the rest of the model. Test builds
// omit this allowance, so they still report items unused by either path.
#[cfg(feature = "fuzz")]
#[cfg_attr(not(test), allow(dead_code))]
pub mod fuzz;
mod identifier;
mod imports;
mod inputs;
mod load_context;
mod name;
mod package;
mod package_layout;
mod package_resolve;
mod parse;
mod recovery;
mod resolver_db;
mod search;
mod storage;
#[cfg(any(test, feature = "fuzz"))]
mod test_path;
mod workspace;

#[cfg(test)]
mod tests;

pub use db::all_known_files;
pub use db::all_used_files;
pub use db::workspace_files;
pub use db::Db;
pub use db::DbInputs;
pub use db::SourceDb;
pub use definition::Definition;
pub use diagnostic::Annotation;
pub use diagnostic::Diagnostic;
pub use diagnostic::DiagnosticKind;
pub use diagnostic::Severity;
pub use file::File;
pub use file_exports::ExportEntry;
pub use file_exports::FileExports;
pub use file_imports::ImportLayer;
pub use file_revision::FileRevision;
pub use file_source_site::SourceSite;
pub use identifier::Identifier;
pub use identifier::MemberKind;
pub use identifier::NamespaceVisibility;
pub use inputs::LibraryRoots;
pub use inputs::LiveRoot;
pub use inputs::OrphanRoot;
pub use inputs::Root;
pub use inputs::RootKind;
pub use inputs::StaleRoot;
pub use inputs::WorkspaceRoots;
pub use name::Name;
pub use oak_package_metadata::description::Priority;
pub use package::Package;
pub use package_layout::classify_in_package;
pub use package_layout::PackagePlacement;
pub use storage::OakDatabase;
pub use workspace::all_package_dependencies;
