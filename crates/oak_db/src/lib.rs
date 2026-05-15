mod db;
mod file;
mod legacy;
mod name;
mod parse;
mod resolver;
mod source_graph;

#[cfg(test)]
mod tests;

pub use db::Db;
pub use file::File;
pub use legacy::semantic_index_with_source_resolver;
pub use legacy::LegacyDb;
pub use name::Name;
pub use source_graph::package_by_name;
pub use source_graph::script_by_url;
pub use source_graph::FileOwner;
pub use source_graph::LibraryRoots;
pub use source_graph::Package;
pub use source_graph::PackageOrigin;
pub use source_graph::Root;
pub use source_graph::RootKind;
pub use source_graph::Script;
pub use source_graph::WorkspaceRoots;
