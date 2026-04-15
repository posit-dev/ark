use crate::semantic_index::NseScope;
use crate::semantic_index::ScopeLaziness;

/// Annotation describing how an NSE function's arguments create scopes.
#[derive(Debug)]
pub struct NseAnnotation {
    pub scoped_args: &'static [ScopedArg],
}

/// A single argument that creates an NSE scope.
#[derive(Debug)]
pub struct ScopedArg {
    pub name: &'static str,
    pub position: usize,
    pub nse_scope: NseScope,
    pub laziness: ScopeLaziness,
}

struct Entry {
    package: &'static str,
    function: &'static str,
    annotation: NseAnnotation,
}

static REGISTRY: &[Entry] = &[
    // base
    Entry {
        package: "base",
        function: "eval",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Current,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    Entry {
        package: "base",
        function: "evalq",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Current,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    Entry {
        package: "base",
        function: "local",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    Entry {
        package: "base",
        function: "with",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 1,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    Entry {
        package: "base",
        function: "with.default",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 1,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    Entry {
        package: "base",
        function: "within",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 1,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    Entry {
        package: "base",
        function: "within.data.frame",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 1,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    // rlang
    Entry {
        package: "rlang",
        function: "on_load",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Current,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    // shiny
    Entry {
        package: "shiny",
        function: "observe",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "x",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    Entry {
        package: "shiny",
        function: "reactive",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "x",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    Entry {
        package: "shiny",
        function: "renderPlot",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    Entry {
        package: "shiny",
        function: "renderPrint",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    Entry {
        package: "shiny",
        function: "renderTable",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    Entry {
        package: "shiny",
        function: "renderText",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    Entry {
        package: "shiny",
        function: "renderUI",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "expr",
                position: 0,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Lazy,
            }],
        },
    },
    // testthat
    Entry {
        package: "testthat",
        function: "test_that",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "code",
                position: 1,
                nse_scope: NseScope::Nested,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    // withr
    Entry {
        package: "withr",
        function: "with_dir",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "code",
                position: 1,
                nse_scope: NseScope::Current,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
    Entry {
        package: "withr",
        function: "with_options",
        annotation: NseAnnotation {
            scoped_args: &[ScopedArg {
                name: "code",
                position: 1,
                nse_scope: NseScope::Current,
                laziness: ScopeLaziness::Eager,
            }],
        },
    },
];

/// Look up the NSE annotation for a `(package, function)` pair.
pub fn lookup(package: &str, function: &str) -> Option<&'static NseAnnotation> {
    REGISTRY
        .iter()
        .find(|e| e.package == package && e.function == function)
        .map(|e| &e.annotation)
}

/// Look up an NSE annotation by function name alone, across all packages.
/// Used for bare unbound calls like `test_that(...)` where the package is
/// unknown. Returns `None` if the name is ambiguous (multiple packages).
pub fn lookup_by_name(function: &str) -> Option<&'static NseAnnotation> {
    let mut found: Option<&'static NseAnnotation> = None;
    for entry in REGISTRY.iter() {
        if entry.function == function {
            if found.is_some() {
                return None;
            }
            found = Some(&entry.annotation);
        }
    }
    found
}
