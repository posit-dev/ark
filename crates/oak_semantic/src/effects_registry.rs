use crate::effects::NseAnnotation;
use crate::effects::NseArgument;
use crate::semantic_index::NseScope::Current;
use crate::semantic_index::NseScope::Nested;
use crate::semantic_index::NseTiming::Eager;
use crate::semantic_index::NseTiming::Lazy;

struct Entry {
    package: &'static str,
    function: &'static str,
    annotation: NseAnnotation,
}

/// Look up the NSE annotation for a `(package, function)` pair.
pub fn lookup(package: &str, function: &str) -> Option<&'static NseAnnotation> {
    REGISTRY
        .iter()
        .find(|e| e.package == package && e.function == function)
        .map(|e| &e.annotation)
}

/// Whether any registry entry annotates `name`. This is the bare-callee front
/// gate: an unannotated name can't resolve to an effect no matter which provider
/// wins, so recognition skips resolution entirely.
pub fn is_annotated(name: &str) -> bool {
    REGISTRY.iter().any(|e| e.function == name)
}

/// One registry entry. Each `(name, position, scope, laziness)` tuple is a
/// scoped argument; list more than one for a function that scopes several.
macro_rules! entry {
    ($pkg:literal, $func:literal, $(($name:literal, $pos:literal, $scope:expr, $timing:expr)),+ $(,)?) => {
        Entry {
            package: $pkg,
            function: $func,
            annotation: NseAnnotation {
                arguments: &[$(NseArgument {
                    name: $name,
                    position: $pos,
                    scope: $scope,
                    timing: $timing,
                }),+],
            },
        }
    };
}

static REGISTRY: &[Entry] = &[
    // base
    entry!("base", "evalq", ("expr", 0, Current, Eager)),
    entry!("base", "local", ("expr", 0, Nested, Eager)),
    entry!("base", "with", ("expr", 1, Nested, Eager)),
    entry!("base", "with.default", ("expr", 1, Nested, Eager)),
    entry!("base", "within", ("expr", 1, Nested, Eager)),
    entry!("base", "within.data.frame", ("expr", 1, Nested, Eager)),
    // rlang
    entry!("rlang", "on_load", ("expr", 0, Current, Lazy)),
    // shiny
    entry!("shiny", "observe", ("x", 0, Nested, Lazy)),
    entry!("shiny", "reactive", ("x", 0, Nested, Lazy)),
    entry!("shiny", "renderPlot", ("expr", 0, Nested, Lazy)),
    entry!("shiny", "renderPrint", ("expr", 0, Nested, Lazy)),
    entry!("shiny", "renderTable", ("expr", 0, Nested, Lazy)),
    entry!("shiny", "renderText", ("expr", 0, Nested, Lazy)),
    entry!("shiny", "renderUI", ("expr", 0, Nested, Lazy)),
    // testthat
    entry!("testthat", "test_that", ("code", 1, Nested, Eager)),
];
