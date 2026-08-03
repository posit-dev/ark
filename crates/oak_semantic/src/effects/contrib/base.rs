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

pub(crate) static ENTRIES: &[Entry] = &[
    // base NSE
    nse!("base", "evalq", ("expr", 0, Current, Eager)),
    nse!("base", "local", ("expr", 0, Nested, Eager)),
    nse!("base", "with", ("expr", 1, Nested, Eager)),
    nse!("base", "with.default", ("expr", 1, Nested, Eager)),
    nse!("base", "within", ("expr", 1, Nested, Eager)),
    nse!("base", "within.data.frame", ("expr", 1, Nested, Eager)),
    // base quote
    quoted!("base", "quote", ("expr", 0)),
    // `bquote` quotes `expr` too, but its `.()` holes escape to evaluation, so
    // it needs a handler rather than a static per-argument effect.
    Entry {
        package: "base",
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
        package: "base",
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
    source!("base", "source", 0),
    // base assign
    assign!("base", "assign", 0),
    assign!("base", "delayedAssign", 0),
];

/// Build the attach [`Entry`] for a base function served by [`LibraryHandler`].
const fn attach_entry(function: &'static str) -> Entry {
    Entry {
        package: "base",
        function,
        effects: EffectsHandlers {
            arguments: None,
            attach: Some(&LibraryHandler),
            source: None,
            assign: None,
        },
    }
}
