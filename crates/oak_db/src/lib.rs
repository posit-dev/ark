mod db;
mod file;
mod files;
mod legacy;
mod name;
mod parse;
mod resolver;
mod root;
mod source_graph;

#[cfg(test)]
mod tests;

pub use db::Db;
pub use file::File;
pub use files::intern_file;
pub use files::Files;
pub use legacy::semantic_index_with_source_resolver;
pub use legacy::LegacyDb;
pub use name::Name;
pub use root::url_to_root;
pub use root::Root;
pub use root::RootKind;
pub use source_graph::Package;
pub use source_graph::PackageOrigin;
pub use source_graph::Script;
pub use source_graph::SourceGraph;
pub use source_graph::SourceNode;
pub use source_graph::WorkspaceRoots;
