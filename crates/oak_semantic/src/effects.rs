use crate::semantic_index::NseScope;
use crate::semantic_index::NseTiming;

/// Effects of a resolved function.
///
/// Currently only records NSE effects. In the future this will include other
/// effects such as `attach` (for e.g. `library()`) and `assign` (for the
/// eponymous function).
#[derive(Debug, Clone, Copy, Default)]
pub struct Effects {
    pub nse: Option<ArgumentsAnnotation>,
}

impl Effects {
    pub fn nse(nse: ArgumentsAnnotation) -> Self {
        Self { nse: Some(nse) }
    }
}

/// Annotation describing how an NSE function's arguments create scopes.
#[derive(Debug, Clone, Copy)]
pub struct ArgumentsAnnotation {
    pub arguments: &'static [Argument],
}

/// A single argument that creates an NSE scope.
#[derive(Debug)]
pub struct Argument {
    pub name: &'static str,
    pub position: usize,
    pub scope: NseScope,
    pub timing: NseTiming,
}
