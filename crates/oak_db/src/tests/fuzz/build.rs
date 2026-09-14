//! Statement constructors shared by the corpus and the generator.

use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Stmt;

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

pub(super) fn library(package: &str) -> Stmt {
    Stmt::effect(
        EffectRecipe::Attach {
            package: package.to_string(),
        },
        Invocation::Bare,
    )
}
