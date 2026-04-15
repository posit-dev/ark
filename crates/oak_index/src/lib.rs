pub mod builder;
pub mod external;
pub(crate) mod index_vec;
pub mod nse_registry;
pub mod semantic_index;
pub mod use_def_map;

pub use builder::semantic_index;
pub use builder::semantic_index_with_nse_resolver;
pub use builder::semantic_index_with_resolvers;
pub use builder::semantic_index_with_source_resolver;
pub use builder::SourceResolution;
pub use semantic_index::DefinitionId;
pub use semantic_index::ScopeId;
pub use semantic_index::UseId;
