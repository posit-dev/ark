//! Statement constructors shared by the corpus and the generator.

use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Stmt;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;

pub(super) fn binding(name: &str) -> Stmt {
    Stmt::bind(name, Expr::Num(1))
}

pub(super) fn function_def(name: &str, body: Vec<Stmt>) -> Stmt {
    Stmt::bind(name, Expr::function(body))
}

/// Binds a callee's name to a function, suppressing its effect for later bare
/// calls to it.
pub(super) fn shadow(name: &str) -> Stmt {
    Stmt::bind(name, Expr::function(vec![]))
}

pub(super) fn source(target: &str) -> Stmt {
    source_with(target, SourceProvider::File, Invocation::Bare)
}

pub(super) fn qualified_source(target: &str) -> Stmt {
    source_with(target, SourceProvider::File, Invocation::Qualified)
}

pub(super) fn source_with(target: &str, provider: SourceProvider, invocation: Invocation) -> Stmt {
    Stmt::effect(
        EffectRecipe::Source {
            path: target.to_string(),
            provider,
        },
        invocation,
    )
}

/// `local()` evaluates its body eagerly in a nested environment, so effects
/// inside it run while the file loads.
pub(super) fn eager_block(body: Vec<Stmt>) -> Stmt {
    Stmt::effect(
        EffectRecipe::Eval {
            env: EvalEnv::Nested,
            timing: EvalTiming::Eager,
            body,
        },
        Invocation::Bare,
    )
}

/// `quote()` never evaluates its body, so nested calls have no effects.
pub(super) fn quoted(body: Vec<Stmt>) -> Stmt {
    Stmt::effect(EffectRecipe::Quote { body }, Invocation::Bare)
}

/// `bquote()` evaluates its hole as ordinary code, so nested effects escape
/// quotation.
pub(super) fn quote_hole(body: Vec<Stmt>) -> Stmt {
    Stmt::effect(
        EffectRecipe::QuoteHoles {
            body: vec![Stmt::Expr(Expr::Hole(body))],
        },
        Invocation::Bare,
    )
}

pub(super) fn library(package: &str) -> Stmt {
    Stmt::effect(
        EffectRecipe::Attach {
            package: package.to_string(),
        },
        Invocation::Bare,
    )
}
