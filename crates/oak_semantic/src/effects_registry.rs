use crate::effects::Argument;
use crate::effects::ArgumentEffect;
use crate::effects::ArgumentsAnnotation;
use crate::effects::AssignAnnotation;
use crate::effects::AttachAnnotation;
use crate::effects::EffectsHandlers;
use crate::effects::SourceAnnotation;
use crate::semantic_index::NseScope::Current;
use crate::semantic_index::NseScope::Nested;
use crate::semantic_index::NseTiming::Eager;
use crate::semantic_index::NseTiming::Lazy;

struct Entry {
    package: &'static str,
    function: &'static str,
    effects: EffectsHandlers,
}

/// Look up the effect handlers of a `(package, function)` pair.
pub fn lookup(package: &str, function: &str) -> Option<&'static EffectsHandlers> {
    REGISTRY
        .iter()
        .find(|e| e.package == package && e.function == function)
        .map(|e| &e.effects)
}

/// Whether any registry entry annotates `name`. This is the bare-callee front
/// gate: an unannotated name can't resolve to an effect no matter which provider
/// wins, so recognition skips resolution entirely.
///
/// TODO: Should be a workspace-wide Salsa-cached query (similar to: does this
/// function dispatches).
pub fn annotates(name: &str) -> bool {
    REGISTRY.iter().any(|e| e.function == name)
}

/// An NSE entry. Each `(name, position, scope, laziness)` tuple is a scoped
/// argument; list more than one for a function that scopes several.
macro_rules! nse {
    ($pkg:literal, $func:literal, $(($name:literal, $pos:literal, $scope:expr, $timing:expr)),+ $(,)?) => {
        Entry {
            package: $pkg,
            function: $func,
            effects: EffectsHandlers {
                arguments: Some(&ArgumentsAnnotation {
                    arguments: &[$(Argument {
                        name: $name,
                        position: $pos,
                        effect: ArgumentEffect::Nse {
                            scope: $scope,
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

/// An attach entry: `(package-argument position, has-`character.only`-flag)`.
macro_rules! attach {
    ($pkg:literal, $func:literal, $pos:literal, $character_only:literal) => {
        Entry {
            package: $pkg,
            function: $func,
            effects: EffectsHandlers {
                arguments: None,
                attach: Some(&AttachAnnotation {
                    character_only: $character_only,
                }),
                source: None,
                assign: None,
            },
        }
    };
}

/// A source entry: `(path-argument position)`. The function reads and evaluates
/// another file, injecting its top-level names into the caller.
macro_rules! source {
    ($pkg:literal, $func:literal, $pos:literal) => {
        Entry {
            package: $pkg,
            function: $func,
            effects: EffectsHandlers {
                arguments: None,
                attach: None,
                source: Some(&SourceAnnotation { position: $pos }),
                assign: None,
            },
        }
    };
}

/// An assign entry: `(name-argument position)`. The function binds a name in the
/// current scope.
macro_rules! assign {
    ($pkg:literal, $func:literal, $pos:literal) => {
        Entry {
            package: $pkg,
            function: $func,
            effects: EffectsHandlers {
                arguments: None,
                attach: None,
                source: None,
                assign: Some(&AssignAnnotation { position: $pos }),
            },
        }
    };
}

static REGISTRY: &[Entry] = &[
    // base NSE
    nse!("base", "evalq", ("expr", 0, Current, Eager)),
    nse!("base", "local", ("expr", 0, Nested, Eager)),
    nse!("base", "with", ("expr", 1, Nested, Eager)),
    nse!("base", "with.default", ("expr", 1, Nested, Eager)),
    nse!("base", "within", ("expr", 1, Nested, Eager)),
    nse!("base", "within.data.frame", ("expr", 1, Nested, Eager)),
    // base attach
    attach!("base", "library", 0, true),
    attach!("base", "require", 0, true),
    // base source
    source!("base", "source", 0),
    // base assign
    assign!("base", "assign", 0),
    assign!("base", "delayedAssign", 0),
    // magrittr / rlang / S7 binding operators
    assign!("magrittr", "%<>%", 0),
    assign!("rlang", "%<~%", 0),
    assign!("S7", ":=", 0),
    // rlang
    nse!("rlang", "on_load", ("expr", 0, Current, Lazy)),
    // shiny
    nse!("shiny", "observe", ("x", 0, Nested, Lazy)),
    nse!("shiny", "reactive", ("x", 0, Nested, Lazy)),
    nse!("shiny", "renderPlot", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderPrint", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderTable", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderText", ("expr", 0, Nested, Lazy)),
    nse!("shiny", "renderUI", ("expr", 0, Nested, Lazy)),
    // testthat
    nse!("testthat", "test_that", ("code", 1, Nested, Eager)),
];
