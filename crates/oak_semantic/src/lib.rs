pub mod builder;
pub mod external;
pub mod library;
pub mod package;
pub mod package_definitions;
pub mod resolver;
pub mod scope_layer;
pub mod semantic_index;
pub mod use_def_map;

pub use builder::build_index;
pub use resolver::ImportsResolver;
pub use resolver::NoopResolver;
pub use resolver::SourceResolution;
pub use semantic_index::DefinitionId;
pub use semantic_index::ScopeId;
pub use semantic_index::UseId;
