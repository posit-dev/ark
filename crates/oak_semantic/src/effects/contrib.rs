use crate::effects::EffectsHandlers;

mod base;
mod magrittr;
mod rlang;
mod s7;
mod shiny;
mod testthat;

// Fields are read by the query API (`lookup`, `annotates`) in the parent
// `effects` module, hence `pub(super)`.
pub(crate) struct Entry {
    pub(super) package: &'static str,
    pub(super) function: &'static str,
    pub(super) effects: EffectsHandlers,
}

/// An NSE entry. Each `(name, position, scope, laziness)` tuple is a scoped
/// argument; list more than one for a function that scopes several.
macro_rules! nse {
    ($pkg:literal, $func:literal, $(($name:literal, $pos:literal, $scope:expr, $timing:expr)),+ $(,)?) => {
        $crate::effects::contrib::Entry {
            package: $pkg,
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: Some(&$crate::effects::ArgumentsAnnotation {
                    arguments: &[$($crate::effects::Argument {
                        name: $name,
                        position: $pos,
                        effect: $crate::effects::ArgumentEffect::EvalQ {
                            env: $scope,
                            timing: $timing,
                        },
                    }),+],
                }),
                attach: None,
                source: None,
                assign: None,
            },
        }
    };
}
pub(crate) use nse;

/// A quoted entry. Each `(name, position)` names an argument captured
/// unevaluated: its symbols aren't uses and nothing in it runs. `quote`,
/// `bquote`.
macro_rules! quoted {
    ($pkg:literal, $func:literal, $(($name:literal, $pos:literal)),+ $(,)?) => {
        $crate::effects::contrib::Entry {
            package: $pkg,
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: Some(&$crate::effects::ArgumentsAnnotation {
                    arguments: &[$($crate::effects::Argument {
                        name: $name,
                        position: $pos,
                        effect: $crate::effects::ArgumentEffect::Quote,
                    }),+],
                }),
                attach: None,
                source: None,
                assign: None,
            },
        }
    };
}
pub(crate) use quoted;

/// A source entry: `(path-argument position)`. The function reads and evaluates
/// another file, injecting its top-level names into the caller.
macro_rules! source {
    ($pkg:literal, $func:literal, $pos:literal) => {
        $crate::effects::contrib::Entry {
            package: $pkg,
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: None,
                attach: None,
                source: Some(&$crate::effects::SourceAnnotation { position: $pos }),
                assign: None,
            },
        }
    };
}
pub(crate) use source;

/// An assign entry: `(name-argument position)`. The function binds a name in the
/// current scope, naming it in a positional argument it evaluates (`assign("x",
/// v)`).
macro_rules! assign {
    ($pkg:literal, $func:literal, $pos:literal) => {
        $crate::effects::contrib::Entry {
            package: $pkg,
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: None,
                attach: None,
                source: None,
                assign: Some(&$crate::effects::AssignAnnotation { position: $pos }),
            },
        }
    };
}
pub(crate) use assign;

/// An assign-operator entry: a binding operator (`x %<>% f`, `x := v`) that binds
/// a name in the current scope. It captures its LHS unevaluated, so the name
/// comes from the LHS text rather than a positional argument, hence no position.
macro_rules! assign_op {
    ($pkg:literal, $func:literal) => {
        $crate::effects::contrib::Entry {
            package: $pkg,
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: None,
                attach: None,
                source: None,
                assign: Some(&$crate::effects::BindingOperatorHandler),
            },
        }
    };
}
pub(crate) use assign_op;

pub(super) static REGISTRY: &[&[Entry]] = &[
    base::ENTRIES,
    magrittr::ENTRIES,
    rlang::ENTRIES,
    s7::ENTRIES,
    shiny::ENTRIES,
    testthat::ENTRIES,
];
