use crate::effects::EffectsHandlers;

mod base;
mod magrittr;
mod rlang;
mod s7;
mod shiny;
mod targets;
mod testthat;
mod withr;

// Fields are read by the query API (`lookup`, `annotates`) in the parent
// `effects` module, hence `pub(super)`.
pub(crate) struct Entry {
    pub(super) function: &'static str,
    pub(super) effects: EffectsHandlers,
}

/// A package's function entries, grouped under the name they all share.
pub(crate) struct PackageEntries {
    pub(super) name: &'static str,
    pub(super) functions: &'static [Entry],
}

/// An NSE entry. Each `(name, position, scope, laziness)` tuple is a scoped
/// argument; list more than one for a function that scopes several.
macro_rules! nse {
    ($func:literal, $(($name:literal, $pos:literal, $scope:expr, $timing:expr)),+ $(,)?) => {
        $crate::effects::contrib::Entry {
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
    ($func:literal, $(($name:literal, $pos:literal)),+ $(,)?) => {
        $crate::effects::contrib::Entry {
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

/// A source entry: `(path-argument position)`, optionally what that argument
/// names (a [`SourceTarget`] variant, `File` by default) and what the function
/// reads when called with no arguments.
///
/// [`SourceTarget`]: crate::effects::SourceTarget
macro_rules! source {
    ($func:literal, $pos:literal) => {
        $crate::effects::contrib::source!($func, $pos, File, None)
    };
    ($func:literal, $pos:literal, $target:ident, $default:expr) => {
        $crate::effects::contrib::Entry {
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: None,
                attach: None,
                source: Some(&$crate::effects::SourceAnnotation {
                    position: $pos,
                    target: $crate::effects::SourceTarget::$target,
                    default_path: $default,
                }),
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
    ($func:literal, $pos:literal) => {
        $crate::effects::contrib::Entry {
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

/// An assign-operator entry: `(target access)`. A binding operator (`x %<>% f`,
/// `x := v`) that binds a name in the current scope. It captures its LHS
/// unevaluated, so the name comes from the LHS text rather than a positional
/// argument, hence no position.
macro_rules! assign_op {
    ($func:literal, $target:expr) => {
        $crate::effects::contrib::Entry {
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: None,
                attach: None,
                source: None,
                assign: Some(&$crate::effects::BindingOperatorHandler { target: $target }),
            },
        }
    };
}
pub(crate) use assign_op;

pub(super) static REGISTRY: &[PackageEntries] = &[
    PackageEntries {
        name: "base",
        functions: base::ENTRIES,
    },
    PackageEntries {
        name: "magrittr",
        functions: magrittr::ENTRIES,
    },
    PackageEntries {
        name: "rlang",
        functions: rlang::ENTRIES,
    },
    PackageEntries {
        name: "S7",
        functions: s7::ENTRIES,
    },
    PackageEntries {
        name: "shiny",
        functions: shiny::ENTRIES,
    },
    PackageEntries {
        name: "targets",
        functions: targets::ENTRIES,
    },
    PackageEntries {
        name: "testthat",
        functions: testthat::ENTRIES,
    },
    PackageEntries {
        name: "withr",
        functions: withr::ENTRIES,
    },
];
