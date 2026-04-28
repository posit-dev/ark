pub mod builder;
pub mod external;
pub mod semantic_index;
pub mod use_def_map;

pub use builder::semantic_index;
pub use semantic_index::DefinitionId;
pub use semantic_index::ScopeId;
pub use semantic_index::UseId;
