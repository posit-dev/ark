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

/// Declares non-standard evaluation effects.
macro_rules! nse {
    ($func:literal, $(($name:literal, $scope:expr, $timing:expr)),+ $(,)?) => {
        $crate::effects::contrib::nse!($func, [$($name),+], $(($name, $scope, $timing)),+)
    };
    ($func:literal, [$($formal:literal),+ $(,)?], $(($name:literal, $scope:expr, $timing:expr)),+ $(,)?) => {
        $crate::effects::contrib::Entry {
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: Some(&$crate::effects::ArgumentsAnnotation {
                    formals: &[$($formal),+],
                    arguments: &[$($crate::effects::Argument {
                        name: $name,
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

/// Declares arguments that remain unevaluated.
macro_rules! quoted {
    ($func:literal, $($name:literal),+ $(,)?) => {
        $crate::effects::contrib::quoted!($func, [$($name),+], $($name),+)
    };
    ($func:literal, [$($formal:literal),+ $(,)?], $($name:literal),+ $(,)?) => {
        $crate::effects::contrib::Entry {
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: Some(&$crate::effects::ArgumentsAnnotation {
                    formals: &[$($formal),+],
                    arguments: &[$($crate::effects::Argument {
                        name: $name,
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

/// Declares a source function's signature prefix, path formal, [`SourceTarget`],
/// and no-argument default. The target defaults to [`SourceTarget::File`].
///
/// [`SourceTarget`]: crate::effects::SourceTarget
/// [`SourceTarget::File`]: crate::effects::SourceTarget::File
macro_rules! source {
    ($func:literal, [$($formal:literal),+ $(,)?], $path:literal) => {
        $crate::effects::contrib::source!(
            $func,
            [$($formal),+],
            $path,
            $crate::effects::SourceTarget::File,
            None
        )
    };
    ($func:literal, [$($formal:literal),+ $(,)?], $path:literal, $target:expr, $default:expr) => {
        $crate::effects::contrib::Entry {
            function: $func,
            effects: $crate::effects::EffectsHandlers {
                arguments: None,
                attach: None,
                source: Some(&$crate::effects::SourceAnnotation {
                    formals: &[$($formal),+],
                    path: $path,
                    target: $target,
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
