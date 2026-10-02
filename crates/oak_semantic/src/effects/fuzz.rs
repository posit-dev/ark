//! Abstract effects rendered in generated fuzz programs.
//!
//! Mutually recursive with [`crate::fuzz`]: an effect's body holds
//! statements, and a statement can hold an effect.

use crate::effects::TargetAccess;
use crate::fuzz::Block;
use crate::fuzz::Expr;
use crate::fuzz::Invocation;
use crate::fuzz::Renderer;
use crate::semantic_index::EvalEnv;
use crate::semantic_index::EvalTiming;

/// A code source recipe for an [`effect`](crate::effects::Effects).
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EffectRecipe {
    Source {
        path: String,
        provider: SourceProvider,
    },
    Attach {
        package: String,
    },
    Assign {
        name: String,
        value: Expr,
    },
    Rebind {
        name: String,
        value: Expr,
        target: TargetAccess,
    },
    Eval {
        env: EvalEnv,
        timing: EvalTiming,
        body: Block,
    },
    Quote {
        body: Block,
    },
    QuoteHoles {
        body: Block,
    },
    Substitute {
        body: Block,
    },
}

/// Which source function renders, and so which [`SourceTarget`] the registry
/// annotation carries. The walk is fixed by the provider rather than chosen
/// separately, because no registry entry declares `Dir(Recursive)` or
/// `FileOrDir(Shallow)`.
///
/// [`SourceTarget`]: crate::effects::SourceTarget
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SourceProvider {
    /// `source()`, taking a file.
    File,
    /// `sourceDir()`, taking a directory, shallow.
    Dir,
    /// `tar_source()`, taking either, recursive. `path` decides which.
    FileOrDir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Callee {
    /// `None` for `sourceDir`, a user-defined idiom with no package to name.
    pub package: Option<&'static str>,
    pub name: &'static str,
    pub form: Form,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Call,
    Infix,
}

/// Returns the representative callee for a recipe. The exhaustive match makes
/// every new [`EffectRecipe`] choose a callee.
pub fn callee(recipe: &EffectRecipe) -> Callee {
    match recipe {
        EffectRecipe::Source {
            provider: SourceProvider::File,
            ..
        } => Callee {
            package: Some("base"),
            name: "source",
            form: Form::Call,
        },
        EffectRecipe::Source {
            provider: SourceProvider::Dir,
            ..
        } => Callee {
            package: None,
            name: "sourceDir",
            form: Form::Call,
        },
        EffectRecipe::Source {
            provider: SourceProvider::FileOrDir,
            ..
        } => Callee {
            package: Some("targets"),
            name: "tar_source",
            form: Form::Call,
        },
        EffectRecipe::Attach { .. } => Callee {
            package: Some("base"),
            name: "library",
            form: Form::Call,
        },
        EffectRecipe::Assign { .. } => Callee {
            package: Some("base"),
            name: "assign",
            form: Form::Call,
        },
        // `:=` is WALRUS and `%<>%` is SPECIAL, the two token kinds accepted
        // by `resolve_operator_assign()`.
        EffectRecipe::Rebind {
            target: TargetAccess::Write,
            ..
        } => Callee {
            package: Some("S7"),
            name: ":=",
            form: Form::Infix,
        },
        EffectRecipe::Rebind {
            target: TargetAccess::ReadWrite,
            ..
        } => Callee {
            package: Some("magrittr"),
            name: "%<>%",
            form: Form::Infix,
        },
        EffectRecipe::Eval {
            env: EvalEnv::Current,
            timing: EvalTiming::Eager,
            ..
        } => Callee {
            package: Some("base"),
            name: "evalq",
            form: Form::Call,
        },
        EffectRecipe::Eval {
            env: EvalEnv::Nested,
            timing: EvalTiming::Eager,
            ..
        } => Callee {
            package: Some("base"),
            name: "local",
            form: Form::Call,
        },
        EffectRecipe::Eval {
            env: EvalEnv::Current,
            timing: EvalTiming::Lazy,
            ..
        } => Callee {
            package: Some("base"),
            name: "on.exit",
            form: Form::Call,
        },
        EffectRecipe::Eval {
            env: EvalEnv::Nested,
            timing: EvalTiming::Lazy,
            ..
        } => Callee {
            package: Some("shiny"),
            name: "reactive",
            form: Form::Call,
        },
        EffectRecipe::Quote { .. } => Callee {
            package: Some("base"),
            name: "quote",
            form: Form::Call,
        },
        EffectRecipe::QuoteHoles { .. } => Callee {
            package: Some("base"),
            name: "bquote",
            form: Form::Call,
        },
        EffectRecipe::Substitute { .. } => Callee {
            package: Some("base"),
            name: "substitute",
            form: Form::Call,
        },
    }
}

/// A call is qualified only when its callee has a package and call syntax.
/// `sourceDir()` has no package and infix operators cannot use `::`.
fn qualifies(target: &Callee, invocation: Invocation) -> bool {
    invocation == Invocation::Qualified && target.package.is_some() && target.form == Form::Call
}

pub(crate) fn render_effect(
    renderer: &mut Renderer,
    recipe: &EffectRecipe,
    invocation: Invocation,
) {
    let target = callee(recipe);
    match recipe {
        EffectRecipe::Source { path, .. } => {
            render_call(renderer, &target, invocation, |renderer| {
                render_string(renderer, path);
            })
        },
        EffectRecipe::Attach { package } => {
            render_call(renderer, &target, invocation, |renderer| {
                renderer.push_name(package);
            })
        },
        EffectRecipe::Assign { name, value } => {
            render_call(renderer, &target, invocation, |renderer| {
                render_string(renderer, name);
                renderer.push_str(", ");
                renderer.expr(value);
            })
        },
        EffectRecipe::Rebind { name, value, .. } => {
            renderer.push_name(name);
            renderer.push_str(" ");
            renderer.push_str(target.name);
            renderer.push_str(" ");
            renderer.expr(value);
        },
        EffectRecipe::Eval { body, .. } => render_call(renderer, &target, invocation, |renderer| {
            renderer.block(body);
        }),
        EffectRecipe::Quote { body } |
        EffectRecipe::QuoteHoles { body } |
        EffectRecipe::Substitute { body } => {
            render_call(renderer, &target, invocation, |renderer| {
                renderer.block(body);
            })
        },
    }
}

fn render_call(
    renderer: &mut Renderer,
    target: &Callee,
    invocation: Invocation,
    args: impl FnOnce(&mut Renderer),
) {
    if qualifies(target, invocation) {
        if let Some(package) = target.package {
            renderer.push_name(package);
            renderer.push_str("::");
        }
    }
    renderer.push_call_identifier(target.name);
    renderer.push_str("(");
    args(renderer);
    renderer.push_str(")");
}

fn render_string(renderer: &mut Renderer, text: &str) {
    renderer.push_str("\"");
    renderer.push_str(&text.replace('\\', "\\\\").replace('"', "\\\""));
    renderer.push_str("\"");
}
