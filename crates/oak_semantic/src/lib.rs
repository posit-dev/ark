pub mod builder;
pub mod external;
pub mod library;
pub mod package;
pub mod package_definitions;
pub mod scope_layer;
pub mod semantic_index;
pub mod use_def_map;

pub use builder::semantic_index;
pub use builder::semantic_index_with_source_resolver;
pub use builder::SourceResolution;
pub use semantic_index::DefinitionId;
pub use semantic_index::ScopeId;
pub use semantic_index::UseId;
