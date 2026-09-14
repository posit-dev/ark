//! Tests the fuzz program model, renderer, and recorded offsets.

use oak_semantic::effects;
use oak_semantic::effects::fuzz::callee;
use oak_semantic::effects::fuzz::EffectRecipe;
use oak_semantic::effects::fuzz::Form;
use oak_semantic::effects::fuzz::SourceProvider;
use oak_semantic::effects::DirWalk;
use oak_semantic::effects::Effects;
use oak_semantic::effects::ResolvedArgumentEffect;
use oak_semantic::effects::SourceTarget;
use oak_semantic::effects::TargetAccess;
use oak_semantic::fuzz::Expr;
use oak_semantic::fuzz::Invocation;
use oak_semantic::fuzz::Program;
use oak_semantic::fuzz::Stmt;
use oak_semantic::semantic_index::AmbiguityReason;
use oak_semantic::semantic_index::DefinitionKind;
use oak_semantic::semantic_index::EvalEnv;
use oak_semantic::semantic_index::EvalTiming;
use oak_semantic::semantic_index::ScopeId;
use oak_semantic::semantic_index::SemanticCallKind;
use oak_semantic::semantic_index::SemanticDiagnostic;
use oak_semantic::semantic_index::SymbolFlags;
use oak_semantic::semantic_index::UseId;

use crate::common::build_with;
use crate::common::index_with_attached;
use crate::common::index_with_base;
use crate::common::only_assign_def;
use crate::common::semantic_call_kinds;
use crate::resolvers::TestImportsResolver;

fn program(statements: Vec<Stmt>) -> Program {
    Program { statements }
}

fn effect_stmt(recipe: EffectRecipe, invocation: Invocation) -> Stmt {
    Stmt::effect(recipe, invocation)
}

// --- Rendering, one test per Effect variant ---

#[test]
fn test_render_source() {
    let effect = || EffectRecipe::Source {
        path: "a.R".to_string(),
        provider: SourceProvider::File,
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "source(\"a.R\")\n");
    assert_eq!(qualified.render().text, "base::source(\"a.R\")\n");
}

#[test]
fn test_render_source_dir_shallow() {
    // `sourceDir()` has no package, so qualification has no effect.
    let effect = || EffectRecipe::Source {
        path: "a.R".to_string(),
        provider: SourceProvider::Dir,
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "sourceDir(\"a.R\")\n");
    assert_eq!(qualified.render().text, "sourceDir(\"a.R\")\n");
}

#[test]
fn test_render_source_dir_recursive() {
    let effect = || EffectRecipe::Source {
        path: "a.R".to_string(),
        provider: SourceProvider::FileOrDir,
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "tar_source(\"a.R\")\n");
    assert_eq!(qualified.render().text, "targets::tar_source(\"a.R\")\n");
}

#[test]
fn test_render_attach() {
    let effect = || EffectRecipe::Attach {
        package: "pkg".to_string(),
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "library(pkg)\n");
    assert_eq!(qualified.render().text, "base::library(pkg)\n");
}

#[test]
fn test_render_assign() {
    let effect = || EffectRecipe::Assign {
        name: "x".to_string(),
        value: Expr::Num(1),
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "assign(\"x\", 1)\n");
    assert_eq!(qualified.render().text, "base::assign(\"x\", 1)\n");
}

#[test]
fn test_render_rebind_write() {
    // Infix syntax cannot use `::`, so qualification has no effect.
    let effect = || EffectRecipe::Rebind {
        name: "x".to_string(),
        value: Expr::Num(1),
        target: TargetAccess::Write,
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "x := 1\n");
    assert_eq!(qualified.render().text, "x := 1\n");
}

#[test]
fn test_render_rebind_read_write() {
    let effect = || EffectRecipe::Rebind {
        name: "x".to_string(),
        value: Expr::Num(1),
        target: TargetAccess::ReadWrite,
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "x %<>% 1\n");
    assert_eq!(qualified.render().text, "x %<>% 1\n");
}

fn eval_body() -> Vec<Stmt> {
    vec![Stmt::Bind {
        name: "x".to_string(),
        value: Expr::Num(1),
    }]
}

#[test]
fn test_render_eval_current_eager() {
    let effect = || EffectRecipe::Eval {
        env: EvalEnv::Current,
        timing: EvalTiming::Eager,
        body: eval_body(),
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "evalq(x <- 1)\n");
    assert_eq!(qualified.render().text, "base::evalq(x <- 1)\n");
}

#[test]
fn test_render_eval_nested_eager() {
    let effect = || EffectRecipe::Eval {
        env: EvalEnv::Nested,
        timing: EvalTiming::Eager,
        body: eval_body(),
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "local(x <- 1)\n");
    assert_eq!(qualified.render().text, "base::local(x <- 1)\n");
}

#[test]
fn test_render_eval_current_lazy() {
    let effect = || EffectRecipe::Eval {
        env: EvalEnv::Current,
        timing: EvalTiming::Lazy,
        body: eval_body(),
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "on.exit(x <- 1)\n");
    assert_eq!(qualified.render().text, "base::on.exit(x <- 1)\n");
}

#[test]
fn test_render_eval_nested_lazy() {
    let effect = || EffectRecipe::Eval {
        env: EvalEnv::Nested,
        timing: EvalTiming::Lazy,
        body: eval_body(),
    };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "reactive(x <- 1)\n");
    assert_eq!(qualified.render().text, "shiny::reactive(x <- 1)\n");
}

#[test]
fn test_render_quote() {
    let effect = || EffectRecipe::Quote { body: eval_body() };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "quote(x <- 1)\n");
    assert_eq!(qualified.render().text, "base::quote(x <- 1)\n");
}

#[test]
fn test_render_quote_holes() {
    let effect = || EffectRecipe::QuoteHoles { body: eval_body() };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "bquote(x <- 1)\n");
    assert_eq!(qualified.render().text, "base::bquote(x <- 1)\n");
}

#[test]
fn test_render_substitute() {
    let effect = || EffectRecipe::Substitute { body: eval_body() };
    let bare = program(vec![effect_stmt(effect(), Invocation::Bare)]);
    let qualified = program(vec![effect_stmt(effect(), Invocation::Qualified)]);
    assert_eq!(bare.render().text, "substitute(x <- 1)\n");
    assert_eq!(qualified.render().text, "base::substitute(x <- 1)\n");
}

#[test]
fn test_block_with_several_statements_renders_braced() {
    let effect = EffectRecipe::Eval {
        env: EvalEnv::Nested,
        timing: EvalTiming::Eager,
        body: vec![
            Stmt::Bind {
                name: "x".to_string(),
                value: Expr::Num(1),
            },
            Stmt::Expr(Expr::Ident("x".to_string())),
        ],
    };
    let rendered = program(vec![effect_stmt(effect, Invocation::Bare)]).render();
    assert_eq!(rendered.text, "local({\n  x <- 1\n  x\n})\n");
}

#[test]
fn test_block_empty_renders_null() {
    let effect = EffectRecipe::Eval {
        env: EvalEnv::Nested,
        timing: EvalTiming::Eager,
        body: vec![],
    };
    let rendered = program(vec![effect_stmt(effect, Invocation::Bare)]).render();
    assert_eq!(rendered.text, "local(NULL)\n");
}

// --- Offsets ---

#[test]
fn test_first_call_points_at_name_not_package_for_qualified_call() {
    let rendered = program(vec![effect_stmt(
        EffectRecipe::Source {
            path: "b.R".to_string(),
            provider: SourceProvider::File,
        },
        Invocation::Qualified,
    )])
    .render();
    let expected = rendered.text.find("source(\"b.R\")").unwrap();
    assert_eq!(rendered.first_call, Some(expected));
}

#[test]
fn test_first_call_finds_effect_nested_in_function_body() {
    let rendered = program(vec![
        Stmt::Bind {
            name: "val_0".to_string(),
            value: Expr::Num(0),
        },
        Stmt::Bind {
            name: "read".to_string(),
            value: Expr::Function {
                body: vec![effect_stmt(
                    EffectRecipe::Source {
                        path: "a.R".to_string(),
                        provider: SourceProvider::File,
                    },
                    Invocation::Bare,
                )],
            },
        },
    ])
    .render();
    let expected = rendered.text.find("source(\"a.R\")").unwrap();
    assert_eq!(rendered.first_call, Some(expected));
}

#[test]
fn test_first_call_none_for_infix_only_program() {
    let rendered = program(vec![effect_stmt(
        EffectRecipe::Rebind {
            name: "x".to_string(),
            value: Expr::Num(1),
            target: TargetAccess::Write,
        },
        Invocation::Bare,
    )])
    .render();
    assert_eq!(rendered.first_call, None);
}

#[test]
fn test_last_identifier_is_not_inside_the_source_target_string() {
    let rendered = program(vec![
        Stmt::Bind {
            name: "val_0".to_string(),
            value: Expr::Num(1),
        },
        effect_stmt(
            EffectRecipe::Source {
                path: "R/b.R".to_string(),
                provider: SourceProvider::File,
            },
            Invocation::Bare,
        ),
    ])
    .render();
    let offset = rendered.last_identifier.unwrap();
    assert_eq!(&rendered.text[offset..offset + "source".len()], "source");
}

// --- Backticked shadow binding ---

#[test]
fn test_bind_backtick_quotes_non_syntactic_name() {
    let rendered = program(vec![Stmt::Bind {
        name: "%<>%".to_string(),
        value: Expr::Function { body: vec![] },
    }])
    .render();
    assert_eq!(rendered.text, "`%<>%` <- function(...) NULL\n");
}

// --- Coverage gates ---
//
// These fail to compile, rather than fail at runtime, when production grows an
// effect the recipe vocabulary cannot render. They gate the parameter space
// that drives permutation. They say nothing about behaviour added inside an
// existing handler, which only tests and review catch.

/// Destructured without `..` so a new [`Effects`] field forces a decision about
/// whether [`EffectRecipe`] can provoke it.
#[test]
fn test_every_production_effect_field_has_a_recipe() {
    let Effects {
        arguments,
        attach,
        source,
        assign,
    } = Effects::default();

    // arguments: Eval, Quote, QuoteHoles, Substitute. attach: Attach.
    // source: Source. assign: Assign and Rebind.
    assert!(arguments.is_none());
    assert!(attach.is_none());
    assert!(source.is_none());
    assert!(assign.is_none());
}

/// Matched exhaustively so a new [`SourceTarget`] forces a decision. The two
/// unreachable combinations have no provider because no registry entry
/// declares them.
#[test]
fn test_every_reachable_source_target_has_a_provider() {
    let provider = |target| match target {
        SourceTarget::File => Some(SourceProvider::File),
        SourceTarget::Dir(DirWalk::Shallow) => Some(SourceProvider::Dir),
        SourceTarget::FileOrDir(DirWalk::Recursive) => Some(SourceProvider::FileOrDir),
        SourceTarget::Dir(DirWalk::Recursive) | SourceTarget::FileOrDir(DirWalk::Shallow) => None,
    };

    assert_eq!(provider(SourceTarget::File), Some(SourceProvider::File));
    assert_eq!(
        provider(SourceTarget::Dir(DirWalk::Shallow)),
        Some(SourceProvider::Dir)
    );
    assert_eq!(
        provider(SourceTarget::FileOrDir(DirWalk::Recursive)),
        Some(SourceProvider::FileOrDir)
    );
    assert_eq!(provider(SourceTarget::Dir(DirWalk::Recursive)), None);
    assert_eq!(provider(SourceTarget::FileOrDir(DirWalk::Shallow)), None);
}

/// Matched exhaustively so a new [`ResolvedArgumentEffect`] forces a decision.
#[test]
fn test_every_resolved_argument_effect_has_a_recipe() {
    let recipe = |effect| match effect {
        ResolvedArgumentEffect::EvalQ { env, timing } => EffectRecipe::Eval {
            env,
            timing,
            body: vec![],
        },
        // `holes` is what separates the three quotation recipes, and the
        // handler computes it rather than the syntax naming it directly.
        ResolvedArgumentEffect::Quote { .. } => EffectRecipe::Quote { body: vec![] },
    };

    let evalq = recipe(ResolvedArgumentEffect::EvalQ {
        env: EvalEnv::Nested,
        timing: EvalTiming::Lazy,
    });
    assert_eq!(callee(&evalq).name, "reactive");

    let quote = recipe(ResolvedArgumentEffect::Quote { holes: vec![] });
    assert_eq!(callee(&quote).name, "quote");
}

// --- Registry linkage ---

#[test]
fn test_callee_matches_registry_for_every_representative() {
    // One instance of every distinct `Effect` representative, with its
    // expected `(package, name, form)` handwritten independently of
    // `callee()`. This checks the mapping itself: a `callee()` arm that names
    // the wrong representative fails here, not just an entry missing from the
    // registry.
    let cases: [(EffectRecipe, Option<&str>, &str, Form); 14] = [
        (
            EffectRecipe::Source {
                path: "a.R".to_string(),
                provider: SourceProvider::File,
            },
            Some("base"),
            "source",
            Form::Call,
        ),
        (
            EffectRecipe::Source {
                path: "R".to_string(),
                provider: SourceProvider::Dir,
            },
            None,
            "sourceDir",
            Form::Call,
        ),
        (
            EffectRecipe::Source {
                path: "R".to_string(),
                provider: SourceProvider::FileOrDir,
            },
            Some("targets"),
            "tar_source",
            Form::Call,
        ),
        (
            EffectRecipe::Attach {
                package: "pkg".to_string(),
            },
            Some("base"),
            "library",
            Form::Call,
        ),
        (
            EffectRecipe::Assign {
                name: "x".to_string(),
                value: Expr::Num(1),
            },
            Some("base"),
            "assign",
            Form::Call,
        ),
        (
            EffectRecipe::Rebind {
                name: "x".to_string(),
                value: Expr::Num(1),
                target: TargetAccess::Write,
            },
            Some("S7"),
            ":=",
            Form::Infix,
        ),
        (
            EffectRecipe::Rebind {
                name: "x".to_string(),
                value: Expr::Num(1),
                target: TargetAccess::ReadWrite,
            },
            Some("magrittr"),
            "%<>%",
            Form::Infix,
        ),
        (
            EffectRecipe::Eval {
                env: EvalEnv::Current,
                timing: EvalTiming::Eager,
                body: vec![],
            },
            Some("base"),
            "evalq",
            Form::Call,
        ),
        (
            EffectRecipe::Eval {
                env: EvalEnv::Nested,
                timing: EvalTiming::Eager,
                body: vec![],
            },
            Some("base"),
            "local",
            Form::Call,
        ),
        (
            EffectRecipe::Eval {
                env: EvalEnv::Current,
                timing: EvalTiming::Lazy,
                body: vec![],
            },
            Some("base"),
            "on.exit",
            Form::Call,
        ),
        (
            EffectRecipe::Eval {
                env: EvalEnv::Nested,
                timing: EvalTiming::Lazy,
                body: vec![],
            },
            Some("shiny"),
            "reactive",
            Form::Call,
        ),
        (
            EffectRecipe::Quote { body: vec![] },
            Some("base"),
            "quote",
            Form::Call,
        ),
        (
            EffectRecipe::QuoteHoles { body: vec![] },
            Some("base"),
            "bquote",
            Form::Call,
        ),
        (
            EffectRecipe::Substitute { body: vec![] },
            Some("base"),
            "substitute",
            Form::Call,
        ),
    ];

    for (effect, package, name, form) in cases {
        let target = callee(&effect);
        assert_eq!(target.package, package);
        assert_eq!(target.name, name);
        assert_eq!(target.form, form);

        let resolves = match package {
            Some(package) => effects::lookup(package, name).is_some(),
            None => effects::source_dir_idiom(name).is_some(),
        };
        assert!(resolves);
    }
}

// --- Recognition, via the production builder ---

#[test]
fn test_rendered_source_effect_is_recognized() {
    let text = program(vec![effect_stmt(
        EffectRecipe::Source {
            path: "a.R".to_string(),
            provider: SourceProvider::File,
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    let index = index_with_base(&text);
    let kinds = semantic_call_kinds(&index);
    assert_eq!(kinds.len(), 1);
    assert!(matches!(kinds[0], SemanticCallKind::Source { .. }));
}

#[test]
fn test_preceding_bind_of_source_suppresses_bare_source() {
    let text = program(vec![
        Stmt::Bind {
            name: "source".to_string(),
            value: Expr::Function { body: vec![] },
        },
        effect_stmt(
            EffectRecipe::Source {
                path: "a.R".to_string(),
                provider: SourceProvider::File,
            },
            Invocation::Bare,
        ),
    ])
    .render()
    .text;
    let index = index_with_base(&text);
    assert!(semantic_call_kinds(&index).is_empty());
}

#[test]
fn test_preceding_bind_of_source_does_not_suppress_qualified_source() {
    // Qualified calls resolve through their namespace and ignore same-name local bindings.
    let text = program(vec![
        Stmt::Bind {
            name: "source".to_string(),
            value: Expr::Function { body: vec![] },
        },
        effect_stmt(
            EffectRecipe::Source {
                path: "a.R".to_string(),
                provider: SourceProvider::File,
            },
            Invocation::Qualified,
        ),
    ])
    .render()
    .text;
    let index = index_with_base(&text);
    let kinds = semantic_call_kinds(&index);
    assert_eq!(kinds.len(), 1);
    assert!(matches!(kinds[0], SemanticCallKind::Source { .. }));
}

// --- Quote suppression ---

#[test]
fn test_quote_suppresses_nested_effect() {
    // `quote()` does not walk its argument, so nested calls are neither effects nor uses.
    let text = program(vec![effect_stmt(
        EffectRecipe::Quote {
            body: vec![effect_stmt(
                EffectRecipe::Source {
                    path: "a.R".to_string(),
                    provider: SourceProvider::File,
                },
                Invocation::Bare,
            )],
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    let index = index_with_base(&text);
    assert!(semantic_call_kinds(&index).is_empty());
    assert!(index.uses_of("source").is_empty());
}

#[test]
fn test_quote_holes_walks_escaped_source_effect() {
    // A `bquote()` `.()` hole escapes back to evaluation, so an effect placed
    // inside it (rather than a bare call) is walked and recognized the same as
    // ordinary code.
    let text = program(vec![effect_stmt(
        EffectRecipe::QuoteHoles {
            body: vec![Stmt::Expr(Expr::Hole(vec![effect_stmt(
                EffectRecipe::Source {
                    path: "b.R".to_string(),
                    provider: SourceProvider::File,
                },
                Invocation::Bare,
            )]))],
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    assert_eq!(text, "bquote(.(source(\"b.R\")))\n");

    let resolver = TestImportsResolver::with_base().with_source("b.R", &["helper"]);
    let index = build_with(&text, resolver);
    let kinds = semantic_call_kinds(&index);
    assert_eq!(kinds.len(), 1);
    assert!(matches!(kinds[0], SemanticCallKind::Source { .. }));
}

// --- Eager vs deferred shadowing ---

#[test]
fn test_eager_shadow_after_effect_call_is_not_ambiguous() {
    // `local()` is eager, so the later top-level `source` binding cannot shadow its call.
    let text = program(vec![
        effect_stmt(
            EffectRecipe::Eval {
                env: EvalEnv::Nested,
                timing: EvalTiming::Eager,
                body: vec![effect_stmt(
                    EffectRecipe::Source {
                        path: "a.R".to_string(),
                        provider: SourceProvider::File,
                    },
                    Invocation::Bare,
                )],
            },
            Invocation::Bare,
        ),
        Stmt::Bind {
            name: "source".to_string(),
            value: Expr::Function { body: vec![] },
        },
    ])
    .render()
    .text;
    let index = index_with_attached(&text, &["shiny"]);
    assert!(index.diagnostics().is_empty());
}

#[test]
fn test_deferred_shadow_after_effect_call_is_lazy_shadow_ambiguous() {
    // `reactive()` is lazy, so the later top-level binding may shadow `source()`.
    let text = program(vec![
        effect_stmt(
            EffectRecipe::Eval {
                env: EvalEnv::Nested,
                timing: EvalTiming::Lazy,
                body: vec![effect_stmt(
                    EffectRecipe::Source {
                        path: "a.R".to_string(),
                        provider: SourceProvider::File,
                    },
                    Invocation::Bare,
                )],
            },
            Invocation::Bare,
        ),
        Stmt::Bind {
            name: "source".to_string(),
            value: Expr::Function { body: vec![] },
        },
    ])
    .render()
    .text;
    let index = index_with_attached(&text, &["shiny"]);
    let diagnostics = index.diagnostics();
    assert_eq!(diagnostics.len(), 1);
    match &diagnostics[0] {
        SemanticDiagnostic::AmbiguousEffect {
            name,
            reason: AmbiguityReason::LazyShadow { .. },
            ..
        } => assert_eq!(name, "source"),
        other => panic!("unexpected diagnostic: {other:?}"),
    }
}

// --- Escaping and quoting ---

#[test]
fn test_source_path_with_quote_and_backslash_renders_valid_r() {
    // An unescaped `"` or `\` renders a malformed string literal, and
    // `index_with_base()` panics on a syntax error, so reaching the assertions
    // at all is what checks the escaping.
    let text = program(vec![effect_stmt(
        EffectRecipe::Source {
            path: "a\"b\\c.R".to_string(),
            provider: SourceProvider::File,
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    assert_eq!(text, "source(\"a\\\"b\\\\c.R\")\n");

    // `RStringValueExt::string_text()` strips the delimiters and returns the
    // token's raw text, so the path Oak sees keeps the escapes rather than the
    // characters R would resolve them to.
    let index = index_with_base(&text);
    assert_eq!(semantic_call_kinds(&index), [&SemanticCallKind::Source {
        path: "a\\\"b\\\\c.R".into(),
        resolved: None,
    }]);
}

#[test]
fn test_non_syntactic_name_renders_backticked_as_ident_and_call() {
    let text = program(vec![
        Stmt::Expr(Expr::Ident("%<>%".to_string())),
        Stmt::Expr(Expr::Call {
            name: "%<>%".to_string(),
        }),
    ])
    .render()
    .text;
    assert_eq!(text, "`%<>%`\n`%<>%`()\n");
}

#[test]
fn test_backtick_in_name_is_escaped_inside_the_quoting() {
    let text = program(vec![Stmt::Bind {
        name: "a`b\\c".to_string(),
        value: Expr::Num(1),
    }])
    .render()
    .text;
    assert_eq!(text, "`a\\`b\\\\c` <- 1\n");

    // Parses, so the escaping closes the name where it should.
    let index = index_with_base(&text);
    assert_eq!(index.uses_of("source").len(), 0);
}

#[test]
fn test_rebind_target_name_backtick_quoted_when_non_syntactic() {
    let text = program(vec![effect_stmt(
        EffectRecipe::Rebind {
            name: "%<>%".to_string(),
            value: Expr::Num(1),
            target: TargetAccess::Write,
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    assert_eq!(text, "`%<>%` := 1\n");
}

#[test]
fn test_attach_package_backtick_quoted_when_non_syntactic() {
    let text = program(vec![effect_stmt(
        EffectRecipe::Attach {
            package: "%<>%".to_string(),
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    assert_eq!(text, "library(`%<>%`)\n");
}

#[test]
fn test_leading_dot_digit_renders_backticked() {
    // `.1` lexes as a number in R, not a name, so it needs backticks.
    let text = program(vec![Stmt::Expr(Expr::Ident(".1".to_string()))])
        .render()
        .text;
    assert_eq!(text, "`.1`\n");
}

#[test]
fn test_reserved_word_renders_backticked() {
    let text = program(vec![Stmt::Expr(Expr::Ident("if".to_string()))])
        .render()
        .text;
    assert_eq!(text, "`if`\n");
}

// --- Semantic behavior of rendered effects ---

#[test]
fn test_assign_effect_records_definition_and_resolves_later_use() {
    // `assign("x", 1)` binds `x`, and a later use resolves to that binding.
    let text = program(vec![
        effect_stmt(
            EffectRecipe::Assign {
                name: "x".to_string(),
                value: Expr::Num(1),
            },
            Invocation::Bare,
        ),
        Stmt::Expr(Expr::Ident("x".to_string())),
    ])
    .render()
    .text;
    let index = index_with_base(&text);
    let file = ScopeId::from(0);

    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));

    let map = index.use_def_map(file);
    let bindings = map.bindings_at_use(UseId::from(1));
    assert_eq!(bindings.definitions().len(), 1);
    let def = &index.definitions(file)[bindings.definitions()[0]];
    assert!(matches!(def.kind(), DefinitionKind::Assign { .. }));
}

#[test]
fn test_rebind_read_write_records_definition_and_use_of_target() {
    // `%<>%` expands to `x <- x %>% f()`, so `x` is both a definition and a use.
    let text = program(vec![effect_stmt(
        EffectRecipe::Rebind {
            name: "x".to_string(),
            value: Expr::Num(1),
            target: TargetAccess::ReadWrite,
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    let index = index_with_attached(&text, &["magrittr"]);
    let file = ScopeId::from(0);

    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_rebind_write_records_definition_without_use_of_target() {
    // S7's `:=` is a pure binding operator (`x <- expr`, not compound), so it
    // reads only its right operand: a definition, but no use of `x`.
    let text = program(vec![effect_stmt(
        EffectRecipe::Rebind {
            name: "x".to_string(),
            value: Expr::Num(1),
            target: TargetAccess::Write,
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    let index = index_with_attached(&text, &["S7"]);
    let file = ScopeId::from(0);

    assert!(matches!(
        only_assign_def(&index),
        Some(DefinitionKind::Assign { .. })
    ));
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_eval_current_binding_leaks_to_file_scope() {
    // `evalq()` is `Current` + `Eager`: no scope push, so its binding lands
    // directly in file scope.
    let text = program(vec![effect_stmt(
        EffectRecipe::Eval {
            env: EvalEnv::Current,
            timing: EvalTiming::Eager,
            body: vec![Stmt::Bind {
                name: "x".to_string(),
                value: Expr::Num(1),
            }],
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    let index = index_with_base(&text);
    let file = ScopeId::from(0);

    assert_eq!(index.scope_ids().count(), 1);
    assert_eq!(
        index.symbols(file).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_eval_nested_binding_does_not_leak_to_file_scope() {
    // `local()` is `Nested`, so its binding stays in the pushed scope and does
    // not leak to file scope.
    let text = program(vec![effect_stmt(
        EffectRecipe::Eval {
            env: EvalEnv::Nested,
            timing: EvalTiming::Eager,
            body: vec![Stmt::Bind {
                name: "x".to_string(),
                value: Expr::Num(1),
            }],
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    let index = index_with_base(&text);
    let file = ScopeId::from(0);
    let local_scope = ScopeId::from(1);

    assert!(index.symbols(file).get("x").is_none());
    assert_eq!(
        index.symbols(local_scope).get("x").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}

#[test]
fn test_substitute_in_local_frame_is_a_use() {
    // Inside `local()`, `substitute` substitutes the frame's own bindings, so a
    // name the local body binds is reported as a use.
    let text = program(vec![effect_stmt(
        EffectRecipe::Eval {
            env: EvalEnv::Nested,
            timing: EvalTiming::Eager,
            body: vec![
                Stmt::Bind {
                    name: "y".to_string(),
                    value: Expr::Num(1),
                },
                effect_stmt(
                    EffectRecipe::Substitute {
                        body: vec![Stmt::Expr(Expr::Ident("y".to_string()))],
                    },
                    Invocation::Bare,
                ),
            ],
        },
        Invocation::Bare,
    )])
    .render()
    .text;
    let index = index_with_base(&text);
    let local_scope = ScopeId::from(1);

    assert_eq!(
        index.symbols(local_scope).get("y").unwrap().flags(),
        SymbolFlags::IS_BOUND.union(SymbolFlags::IS_USED)
    );
}

#[test]
fn test_substitute_at_file_scope_quotes_without_use() {
    // R substitutes nothing in the global environment, so a top-level
    // `substitute` leaves its argument quoted: `a` stays bound-only.
    let text = program(vec![
        Stmt::Bind {
            name: "a".to_string(),
            value: Expr::Num(1),
        },
        effect_stmt(
            EffectRecipe::Substitute {
                body: vec![Stmt::Expr(Expr::Ident("a".to_string()))],
            },
            Invocation::Bare,
        ),
    ])
    .render()
    .text;
    let index = index_with_base(&text);
    let file = ScopeId::from(0);

    assert_eq!(
        index.symbols(file).get("a").unwrap().flags(),
        SymbolFlags::IS_BOUND
    );
}
