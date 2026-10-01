mod bquote;
mod c;
mod library;
mod substitute;

use bquote::BquoteHandler;
use c::CHandler;
use library::LibraryHandler;
use substitute::SubstituteHandler;

use crate::effects::contrib::assign;
use crate::effects::contrib::nse;
use crate::effects::contrib::quoted;
use crate::effects::contrib::source;
use crate::effects::contrib::Entry;
use crate::effects::EffectsHandlers;
use crate::effects::FunctionHandlers;
use crate::semantic_index::EvalEnv::Current;
use crate::semantic_index::EvalEnv::Nested;
use crate::semantic_index::EvalTiming::Eager;
use crate::semantic_index::EvalTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    // base NSE
    nse!("evalq", ("expr", Current, Eager)),
    // `on.exit()` evaluates its captured `expr` in the function frame when it
    // exits, so its effect is lazy in the current scope.
    nse!("on.exit", ("expr", Current, Lazy)),
    nse!("local", ("expr", Nested, Eager)),
    nse!("with", ["data", "expr"], ("expr", Nested, Eager)),
    nse!("with.default", ["data", "expr"], ("expr", Nested, Eager)),
    nse!("within", ["data", "expr"], ("expr", Nested, Eager)),
    nse!(
        "within.data.frame",
        ["data", "expr"],
        ("expr", Nested, Eager)
    ),
    // base quote
    quoted!("quote", "expr"),
    // `bquote` quotes `expr` too, but its `.()` holes escape to evaluation, so
    // it needs a handler rather than a static per-argument effect.
    Entry {
        function: "bquote",
        handlers: FunctionHandlers::with_effects(EffectsHandlers {
            arguments: Some(&BquoteHandler),
            ..EffectsHandlers::EMPTY
        }),
    },
    // `substitute` quotes `expr` too, but replaces the symbols its environment
    // binds, so it needs a handler that queries the scope rather than a static
    // per-argument effect.
    Entry {
        function: "substitute",
        handlers: FunctionHandlers::with_effects(EffectsHandlers {
            arguments: Some(&SubstituteHandler),
            ..EffectsHandlers::EMPTY
        }),
    },
    // base attach. `library`/`require` share `LibraryHandler` (below).
    attach_entry("library"),
    attach_entry("require"),
    // base source
    source!("source", ["file", "local"], "file"),
    // base assign
    assign!(
        "assign",
        ["x", "value", "pos", "envir", "inherits", "immediate"],
        "x",
        "value",
        ["pos", "envir"]
    ),
    assign!(
        "delayedAssign",
        ["x", "value", "eval.env", "assign.env"],
        "x",
        "value",
        ["assign.env"]
    ),
    Entry {
        function: "c",
        handlers: FunctionHandlers::with_value(&CHandler),
    },
];

/// Build the attach [`Entry`] for a base function served by [`LibraryHandler`].
const fn attach_entry(function: &'static str) -> Entry {
    Entry {
        function,
        handlers: FunctionHandlers::with_effects(EffectsHandlers {
            attach: Some(&LibraryHandler),
            ..EffectsHandlers::EMPTY
        }),
    }
}
