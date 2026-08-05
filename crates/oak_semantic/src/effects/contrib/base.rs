mod bquote;
mod library;
mod substitute;

use bquote::BquoteHandler;
use library::LibraryHandler;
use substitute::SubstituteHandler;

use crate::effects::contrib::assign;
use crate::effects::contrib::nse;
use crate::effects::contrib::quoted;
use crate::effects::contrib::source;
use crate::effects::contrib::Entry;
use crate::effects::EffectsHandlers;
use crate::semantic_index::EvalEnv::Current;
use crate::semantic_index::EvalEnv::Nested;
use crate::semantic_index::EvalTiming::Eager;
use crate::semantic_index::EvalTiming::Lazy;

pub(crate) static ENTRIES: &[Entry] = &[
    // base NSE
    nse!("evalq", ("expr", Current, Eager)),
    // `on.exit(expr)` captures `expr` and runs it in the current function's
    // frame when the function exits. Bindings land in that frame (`Current`) at
    // an unknown later time (`Lazy`), the same shape as `rlang::on_load()`.
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
        effects: EffectsHandlers {
            arguments: Some(&BquoteHandler),
            attach: None,
            source: None,
            assign: None,
        },
    },
    // `substitute` quotes `expr` too, but replaces the symbols its environment
    // binds, so it needs a handler that queries the scope rather than a static
    // per-argument effect.
    Entry {
        function: "substitute",
        effects: EffectsHandlers {
            arguments: Some(&SubstituteHandler),
            attach: None,
            source: None,
            assign: None,
        },
    },
    // base attach. `library`/`require` share `LibraryHandler` (below).
    attach_entry("library"),
    attach_entry("require"),
    // base source
    source!("source", ["file", "local"], "file"),
    // base assign
    assign!("assign", 0),
    assign!("delayedAssign", 0),
];

/// Build the attach [`Entry`] for a base function served by [`LibraryHandler`].
const fn attach_entry(function: &'static str) -> Entry {
    Entry {
        function,
        effects: EffectsHandlers {
            arguments: None,
            attach: Some(&LibraryHandler),
            source: None,
            assign: None,
        },
    }
}
