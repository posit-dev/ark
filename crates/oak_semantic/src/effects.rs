use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;

/// Effects of a resolved function.
///
/// Currently only records NSE effects. In the future this will include other
/// effects such as `attach` (for e.g. `library()`) and `assign` (for the
/// eponymous function).
#[derive(Debug, Clone, Copy, Default)]
pub struct Effects {
    pub nse: Option<NseAnnotation>,
}

impl Effects {
    pub fn nse(nse: NseAnnotation) -> Self {
        Self { nse: Some(nse) }
    }
}

/// Annotation describing how an NSE function's arguments create scopes.
#[derive(Debug, Clone, Copy)]
pub struct NseAnnotation {
    pub arguments: &'static [NseArgument],
}

/// A single argument that creates an NSE scope.
#[derive(Debug)]
pub struct NseArgument {
    pub name: &'static str,
    pub position: usize,
    pub scope: NseScope,
    pub timing: NseTiming,
}
