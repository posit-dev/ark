//! Parses the `declare()` directive: the first statement of a function body
//! that describes its own calling convention.
//!
//! Two entry shapes coexist, told apart by structure rather than a reserved
//! word. A named argument (`x = Quote`) is arg-centric: it says how one
//! formal's own sub-expressions evaluate. A positional call
//! (`Attach(.(pkg))`) is effect-centric: it says what the call does to the
//! environment, reading its operands from `.()` holes.
//!
//! Inside those holes, only one thing is live: `.()` means evaluate. A bare
//! `substitute(x)` without the `.()` wrapper is inert structure, not an
//! operand. That's the one escape mechanism, and there are no others.

use aether_syntax::AnyRExpression;
use aether_syntax::AnyRParameterName;
use aether_syntax::RArgument;
use aether_syntax::RCall;
use aether_syntax::RCallArguments;
use aether_syntax::RFunctionDefinition;
use aether_syntax::RIfStatement;
use aether_syntax::RParameter;
use aether_syntax::RParameters;
use biome_rowan::AstNode;
use biome_rowan::AstNodeList;
use biome_rowan::AstSeparatedList;
use biome_rowan::TextRange;
use oak_core::declaration::as_declare_args;
use oak_core::syntax_ext::RIdentifierExt;

use crate::effects::declaration::argument_name;
use crate::effects::declaration::argument_values;
use crate::effects::declaration::match_signature;
use crate::effects::ArgumentEffect;
use crate::effects::ArgumentRef;
use crate::effects::CallContext;
use crate::effects::DeclExpr;
use crate::effects::Declaration;
use crate::effects::EnvOp;
use crate::effects::EnvParent;
use crate::effects::EnvironmentEffect;
use crate::effects::EvalMode;
use crate::effects::FormalDef;
use crate::effects::RExpr;
use crate::effects::StaticValue;
use crate::semantic_index::NseTiming;

/// A [`Declaration`] parsed from a `declare()` directive, plus the
/// diagnostics collected along the way. Malformed entries are dropped
/// individually, so the declaration always reflects what parsed, not an
/// all-or-nothing result.
pub struct ParsedDeclaration {
    pub declaration: Declaration,
    pub diagnostics: Vec<DeclareDiagnostic>,
}

/// One thing that went wrong while parsing a `declare()` directive. The
/// range points at the offending node, for a caller to surface as a
/// diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclareDiagnostic {
    pub kind: DeclareDiagnosticKind,
    pub range: TextRange,
}

impl DeclareDiagnostic {
    fn new(kind: DeclareDiagnosticKind, range: TextRange) -> Self {
        DeclareDiagnostic { kind, range }
    }
}

/// What went wrong. Each variant carries what a later lint message needs to
/// name the problem, typically the offending name; the node it happened at
/// is on [`DeclareDiagnostic::range`] instead of repeated in every variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclareDiagnosticKind {
    /// An arg-centric entry (`name = ...`) names a formal the signature
    /// doesn't have, or a `.()` hole references one.
    UnknownFormal { name: String },
    /// An arg-centric entry's RHS isn't `Quote`, `Quote()`, `Nse`, or
    /// `Nse(...)`, or a constructor call has the wrong argument shape (e.g.
    /// `Quote(x)`).
    InvalidEvalMode,
    /// `Nse`'s `eager` argument isn't a bare bool. A malformed `scope` operand
    /// surfaces as `InvalidOperand` from `parse_decl_expr` instead.
    InvalidNseArgument { name: String },
    /// A positional entry's call head isn't a recognized effect
    /// (`Attach`/`Source`), or the entry isn't a call at all.
    UnknownEffect { name: String },
    /// An operand position isn't `.()`/`if` shaped, or a `.()` hole's
    /// contents fall outside the bounded interpreted set (identifier
    /// naming a formal, or `substitute()` of one).
    InvalidOperand,
    /// An `if` operand is missing its `else` branch.
    MissingElse,
    /// The same formal is named by more than one arg-centric entry; the
    /// first one parsed wins.
    DuplicateEntry { name: String },
    /// An operand forces (`Eval`s) a formal that an arg-centric entry
    /// declares `Quote` or `Nse`, so the two entries disagree about
    /// whether the formal is captured.
    ContradictoryLiveness { name: String },
}

/// Parse the `declare()` directive at the top of `function`'s body, or
/// `None` if it has none.
///
/// The directive must be the function's first statement (comments aside):
/// either the body is a braced block whose first expression is the
/// directive, or the body IS the directive. A `declare()` anywhere else in
/// the body isn't this function's declaration and isn't inspected here.
pub fn parse_declaration(function: &RFunctionDefinition) -> Option<ParsedDeclaration> {
    let args = directive_args(function)?;
    let params = function.parameters().ok()?;
    let formals = build_formals(&params);

    let mut diagnostics = Vec::new();
    let mut arguments = Vec::new();
    // Paired with the entry's range so the contradiction pass below can
    // still point at the effect that forced the disagreement, without
    // threading a parallel range vector through every callee.
    let mut env: Vec<(EnvironmentEffect, TextRange)> = Vec::new();

    for item in args.items().iter() {
        let Ok(arg) = item else { continue };
        // Structural disambiguation: a named argument is always arg-centric,
        // even when its name happens to be an effect keyword (`Attach = Quote`
        // on a formal named `Attach`), because only a positional call can be
        // effect-centric.
        if arg.name_clause().is_some() {
            parse_arg_centric_entry(&arg, &formals, &mut arguments, &mut diagnostics);
        } else {
            parse_effect_entry(&arg, &formals, &mut env, &mut diagnostics);
        }
    }

    check_contradictions(&env, &arguments, &formals, &mut diagnostics);

    let declaration = Declaration {
        formals,
        arguments,
        env: env.into_iter().map(|(effect, _)| effect).collect(),
    };
    Some(ParsedDeclaration {
        declaration,
        diagnostics,
    })
}

/// The `declare()` call's arguments, if `function`'s body starts with one.
fn directive_args(function: &RFunctionDefinition) -> Option<RCallArguments> {
    let body = function.body().ok()?;
    let first_statement = match body {
        AnyRExpression::RBracedExpressions(braced) => braced.expressions().iter().next()?,
        other => other,
    };
    as_declare_args(&first_statement)
}

/// One [`FormalDef`] per parameter in signature order, `...` and `..i`
/// included. A `TRUE`/`FALSE` default becomes a [`StaticValue::Bool`] and an
/// env-capture op (`parent.frame()`, `new.env()`) a [`StaticValue::Env`];
/// anything else (a call like `getOption(...)`, or no default) leaves it `None`.
fn build_formals(params: &RParameters) -> Vec<FormalDef> {
    params
        .items()
        .iter()
        .filter_map(|param| param.ok())
        .map(|param| {
            let name = parameter_name(&param);
            let default = param
                .default()
                .and_then(|default| default.value().ok())
                .and_then(|value| static_default(&value));
            FormalDef { name, default }
        })
        .collect()
}

/// Read a formal's default as a [`StaticValue`]. A `TRUE`/`FALSE` is a
/// [`StaticValue::Bool`]; otherwise a recognized env-capture op is a
/// [`StaticValue::Env`]. Anything else stays `None`.
fn static_default(value: &AnyRExpression) -> Option<StaticValue> {
    if let Some(bool) = CallContext::new().resolve_static_bool(value) {
        return Some(StaticValue::Bool(bool));
    }
    recognize_env_op(value).map(StaticValue::Env)
}

/// A parameter's name, matching the naming `scan_parameter_defaults` in
/// `builder.rs` already uses: `...` for dots, the trimmed token text (e.g.
/// `..1`) for a `..i` parameter.
fn parameter_name(param: &RParameter) -> String {
    match param.name() {
        Ok(AnyRParameterName::RIdentifier(ident)) => ident.name_text(),
        Ok(AnyRParameterName::RDots(_)) => String::from("..."),
        Ok(AnyRParameterName::RDotDotI(ddi)) => ddi.syntax().text_trimmed().to_string(),
        Err(_) => String::new(),
    }
}

/// Parse a named entry (`formal = EvalMode`). Unknown formal, duplicate
/// formal, and unrecognized RHS shape each drop the entry with a
/// diagnostic; a well-formed entry is added to `arguments`.
fn parse_arg_centric_entry(
    arg: &RArgument,
    formals: &[FormalDef],
    arguments: &mut Vec<ArgumentEffect>,
    diagnostics: &mut Vec<DeclareDiagnostic>,
) {
    let range = arg.syntax().text_trimmed_range();
    let Some(name) = argument_name(arg) else {
        return;
    };
    let Some(idx) = formal_index(formals, &name) else {
        diagnostics.push(DeclareDiagnostic::new(
            DeclareDiagnosticKind::UnknownFormal { name },
            range,
        ));
        return;
    };
    // First entry for a formal wins; a later one is a diagnostic, not an
    // overwrite, so the declaration never silently picks a side.
    if arguments.iter().any(|effect| effect.arg.0 == idx) {
        diagnostics.push(DeclareDiagnostic::new(
            DeclareDiagnosticKind::DuplicateEntry { name },
            range,
        ));
        return;
    }
    let Some(value) = arg.value() else {
        return;
    };
    let Some(mode) = parse_eval_mode(&value, formals, diagnostics) else {
        return;
    };
    arguments.push(ArgumentEffect {
        arg: ArgumentRef(idx),
        mode,
    });
}

/// Parse a positional entry (`Effect(...)`). Only `Attach`/`Source` are
/// recognized effect keywords; anything else, or an entry that isn't a
/// call at all, is an unknown effect.
fn parse_effect_entry(
    arg: &RArgument,
    formals: &[FormalDef],
    env: &mut Vec<(EnvironmentEffect, TextRange)>,
    diagnostics: &mut Vec<DeclareDiagnostic>,
) {
    let range = arg.syntax().text_trimmed_range();
    let Some(value) = arg.value() else {
        return;
    };

    let AnyRExpression::RCall(call) = &value else {
        diagnostics.push(unknown_effect(&value, range));
        return;
    };
    let Ok(AnyRExpression::RIdentifier(func)) = call.function() else {
        diagnostics.push(unknown_effect(&value, range));
        return;
    };

    match func.name_text().as_str() {
        "Attach" => parse_attach_call(call, formals, range, env, diagnostics),
        "Source" => parse_source_call(call, formals, range, env, diagnostics),
        _ => diagnostics.push(unknown_effect(&value, range)),
    }
}

fn unknown_effect(value: &AnyRExpression, range: TextRange) -> DeclareDiagnostic {
    let name = value.syntax().text_trimmed().to_string();
    DeclareDiagnostic::new(DeclareDiagnosticKind::UnknownEffect { name }, range)
}

/// `Attach(<declexpr>)`. Matched with [`match_signature`] against a
/// single-formal signature, the same reuse `Source` and `Nse` get, so a
/// named `Attach(pkg = .(x))` works the same as the positional form.
fn parse_attach_call(
    call: &RCall,
    formals: &[FormalDef],
    range: TextRange,
    env: &mut Vec<(EnvironmentEffect, TextRange)>,
    diagnostics: &mut Vec<DeclareDiagnostic>,
) {
    let attach_formals = [FormalDef {
        name: "pkg".to_string(),
        default: None,
    }];
    let matched = match_signature(call, &attach_formals);
    let values = argument_values(call);

    let Some(operand) = bound_value(&matched, &values, 0) else {
        diagnostics.push(invalid_operand(range));
        return;
    };
    let Some(package) = parse_decl_expr(operand, formals, diagnostics) else {
        return;
    };
    env.push((EnvironmentEffect::Attach { package }, range));
}

/// `Source(<declexpr>, envir = <declexpr>)`. `envir` is a full [`DeclExpr`], so
/// it accepts the if/else that maps `local` onto a target scope. It is required
/// (the base stub always writes it); an absent `envir` is an `InvalidOperand`,
/// the same as a missing required operand elsewhere.
fn parse_source_call(
    call: &RCall,
    formals: &[FormalDef],
    range: TextRange,
    env: &mut Vec<(EnvironmentEffect, TextRange)>,
    diagnostics: &mut Vec<DeclareDiagnostic>,
) {
    let source_formals = [
        FormalDef {
            name: "path".to_string(),
            default: None,
        },
        FormalDef {
            name: "envir".to_string(),
            default: None,
        },
    ];
    let matched = match_signature(call, &source_formals);
    let values = argument_values(call);

    let Some(path_operand) = bound_value(&matched, &values, 0) else {
        diagnostics.push(invalid_operand(range));
        return;
    };
    let Some(path) = parse_decl_expr(path_operand, formals, diagnostics) else {
        return;
    };

    let Some(envir_operand) = bound_value(&matched, &values, 1) else {
        diagnostics.push(invalid_operand(range));
        return;
    };
    let Some(envir) = parse_decl_expr(envir_operand, formals, diagnostics) else {
        return;
    };

    env.push((EnvironmentEffect::Source { path, envir }, range));
}

/// The value bound to the formal at `idx`, per `matched` (from
/// [`match_signature`]) and the call's argument values in the same order.
fn bound_value<'a>(
    matched: &[Option<usize>],
    values: &'a [Option<AnyRExpression>],
    idx: usize,
) -> Option<&'a AnyRExpression> {
    let pos = matched.iter().position(|bound| *bound == Some(idx))?;
    values.get(pos)?.as_ref()
}

fn formal_index(formals: &[FormalDef], name: &str) -> Option<usize> {
    formals.iter().position(|formal| formal.name == name)
}

/// Parse an arg-centric RHS: `Quote`, `Quote()`, `Nse`, or `Nse(...)`. `formals`
/// is the enclosing function's signature, so an `Nse` scope operand (`.(envir)`)
/// can resolve to a formal index.
fn parse_eval_mode(
    value: &AnyRExpression,
    formals: &[FormalDef],
    diagnostics: &mut Vec<DeclareDiagnostic>,
) -> Option<EvalMode> {
    let range = value.syntax().text_trimmed_range();
    match value {
        // Bare is an alias for a no-arg call: `Quote` == `Quote()`, `Nse` ==
        // `Nse()`.
        AnyRExpression::RIdentifier(ident) => match ident.name_text().as_str() {
            "Quote" => Some(EvalMode::Quote),
            "Nse" => Some(EvalMode::Nse {
                scope: default_nse_scope(),
                timing: NseTiming::Eager,
            }),
            _ => {
                diagnostics.push(DeclareDiagnostic::new(
                    DeclareDiagnosticKind::InvalidEvalMode,
                    range,
                ));
                None
            },
        },
        AnyRExpression::RCall(call) => {
            let Ok(AnyRExpression::RIdentifier(func)) = call.function() else {
                diagnostics.push(DeclareDiagnostic::new(
                    DeclareDiagnosticKind::InvalidEvalMode,
                    range,
                ));
                return None;
            };
            match func.name_text().as_str() {
                "Quote" => parse_quote_call(call, range, diagnostics),
                "Nse" => parse_nse_call(call, formals, range, diagnostics),
                _ => {
                    diagnostics.push(DeclareDiagnostic::new(
                        DeclareDiagnosticKind::InvalidEvalMode,
                        range,
                    ));
                    None
                },
            }
        },
        _ => {
            diagnostics.push(DeclareDiagnostic::new(
                DeclareDiagnosticKind::InvalidEvalMode,
                range,
            ));
            None
        },
    }
}

/// `Quote()` takes no arguments, so any argument makes the call invalid.
fn parse_quote_call(
    call: &RCall,
    range: TextRange,
    diagnostics: &mut Vec<DeclareDiagnostic>,
) -> Option<EvalMode> {
    let Ok(args) = call.arguments() else {
        diagnostics.push(DeclareDiagnostic::new(
            DeclareDiagnosticKind::InvalidEvalMode,
            range,
        ));
        return None;
    };
    if args.items().iter().count() > 0 {
        diagnostics.push(DeclareDiagnostic::new(
            DeclareDiagnosticKind::InvalidEvalMode,
            range,
        ));
        return None;
    }
    Some(EvalMode::Quote)
}

/// `Nse(scope = new.env(parent = parent.frame()), eager = TRUE)`, matched with
/// [`match_signature`] against that synthetic signature. That's the reuse this
/// vocabulary is designed for: parsing `Nse(.(envir), eager = FALSE)` is the
/// same positional/named resolution as any other call, not a special case.
///
/// `scope` is a full [`DeclExpr`] operand, the same `.()`-hole grammar
/// `Source.envir` uses, so a function's scope can be read from an argument. A
/// bare `Nse()` synthesizes the constructor default `new.env(parent =
/// parent.frame())`, which resolves to a fresh nested scope.
fn parse_nse_call(
    call: &RCall,
    formals: &[FormalDef],
    range: TextRange,
    diagnostics: &mut Vec<DeclareDiagnostic>,
) -> Option<EvalMode> {
    let nse_formals = [
        FormalDef {
            name: "scope".to_string(),
            default: None,
        },
        FormalDef {
            name: "eager".to_string(),
            default: None,
        },
    ];
    let matched = match_signature(call, &nse_formals);
    let values = argument_values(call);

    let scope = match bound_value(&matched, &values, 0) {
        // A malformed operand surfaces as `InvalidOperand` from
        // `parse_decl_expr`, which already pushed the diagnostic.
        Some(operand) => parse_decl_expr(operand, formals, diagnostics)?,
        None => default_nse_scope(),
    };

    let timing = match bound_value(&matched, &values, 1) {
        None => NseTiming::Eager,
        Some(expr) => match CallContext::new().resolve_static_bool(expr) {
            Some(true) => NseTiming::Eager,
            Some(false) => NseTiming::Lazy,
            None => {
                diagnostics.push(DeclareDiagnostic::new(
                    DeclareDiagnosticKind::InvalidNseArgument {
                        name: "eager".to_string(),
                    },
                    range,
                ));
                return None;
            },
        },
    };

    Some(EvalMode::Nse { scope, timing })
}

/// The `Nse` constructor's default scope, `new.env(parent = parent.frame())`,
/// which resolves to a fresh nested scope. Synthesized in Rust rather than
/// parsed, so a bare `Nse()` needs no string surface.
fn default_nse_scope() -> DeclExpr {
    DeclExpr::Hole(RExpr::Env(EnvOp::NewEnv {
        parent: EnvParent::ParentFrame,
    }))
}

/// Parse an effect operand position: either a bare `.()` hole, or an
/// `if`/`else` selecting between two operands on a `.()` condition.
fn parse_decl_expr(
    expr: &AnyRExpression,
    formals: &[FormalDef],
    diagnostics: &mut Vec<DeclareDiagnostic>,
) -> Option<DeclExpr> {
    if let AnyRExpression::RIfStatement(if_stmt) = expr {
        return parse_if_decl_expr(if_stmt, formals, diagnostics);
    }

    let range = expr.syntax().text_trimmed_range();
    let Some(hole) = hole_call_argument(expr) else {
        diagnostics.push(invalid_operand(range));
        return None;
    };
    let rexpr = parse_hole_rexpr(&hole, formals, diagnostics, range)?;
    Some(DeclExpr::Hole(rexpr))
}

/// `if (<hole>) <declexpr> else <declexpr>`. The condition is bounded to a
/// `.()` hole (no comparisons, no general boolean expressions); a missing
/// `else` drops the whole `if`, since there's no branch to fall back to.
fn parse_if_decl_expr(
    if_stmt: &RIfStatement,
    formals: &[FormalDef],
    diagnostics: &mut Vec<DeclareDiagnostic>,
) -> Option<DeclExpr> {
    let range = if_stmt.syntax().text_trimmed_range();

    let condition = if_stmt.condition().ok()?;
    let cond_range = condition.syntax().text_trimmed_range();
    let Some(cond_hole) = hole_call_argument(&condition) else {
        diagnostics.push(invalid_operand(cond_range));
        return None;
    };
    let cond = parse_hole_rexpr(&cond_hole, formals, diagnostics, cond_range)?;

    let consequence = if_stmt.consequence().ok()?;
    let then = parse_decl_expr(&consequence, formals, diagnostics)?;

    let Some(else_clause) = if_stmt.else_clause() else {
        diagnostics.push(DeclareDiagnostic::new(
            DeclareDiagnosticKind::MissingElse,
            range,
        ));
        return None;
    };
    let alternative = else_clause.alternative().ok()?;
    let els = parse_decl_expr(&alternative, formals, diagnostics)?;

    Some(DeclExpr::If {
        cond,
        then: Box::new(then),
        els: Box::new(els),
    })
}

/// The argument of a `.()` hole call: a call to the function `.` with
/// exactly one argument, the same surface bquote's unquote uses. Kept
/// separate from `unquote_hole()` in `contrib/base.rs`, since `declare()`
/// interprets what's inside the hole (a bounded read, not an escaped
/// expression to walk normally) and the two readings shouldn't be coupled.
fn hole_call_argument(expr: &AnyRExpression) -> Option<AnyRExpression> {
    let AnyRExpression::RCall(call) = expr else {
        return None;
    };
    let AnyRExpression::RIdentifier(func) = call.function().ok()? else {
        return None;
    };
    if func.name_text() != "." {
        return None;
    }
    only_argument(call)
}

/// The value of a call's sole argument, or `None` when it doesn't have
/// exactly one.
fn only_argument(call: &RCall) -> Option<AnyRExpression> {
    let args = call.arguments().ok()?;
    let mut items = args.items().iter();
    let only = items.next()?.ok()?;
    if items.next().is_some() {
        return None;
    }
    only.value()
}

fn invalid_operand(range: TextRange) -> DeclareDiagnostic {
    DeclareDiagnostic::new(DeclareDiagnosticKind::InvalidOperand, range)
}

/// The bounded R read inside a `.()` hole: an identifier naming a formal
/// (`.(x)`, forces it), or `substitute()` of one (`.(substitute(x))`,
/// captures it). Anything else, including a nested nonempty expression,
/// falls outside the interpreted set.
fn parse_hole_rexpr(
    expr: &AnyRExpression,
    formals: &[FormalDef],
    diagnostics: &mut Vec<DeclareDiagnostic>,
    range: TextRange,
) -> Option<RExpr> {
    match expr {
        AnyRExpression::RIdentifier(ident) => {
            let name = ident.name_text();
            let Some(idx) = formal_index(formals, &name) else {
                diagnostics.push(DeclareDiagnostic::new(
                    DeclareDiagnosticKind::UnknownFormal { name },
                    range,
                ));
                return None;
            };
            Some(RExpr::Eval(ArgumentRef(idx)))
        },
        AnyRExpression::RCall(call) => {
            let Ok(AnyRExpression::RIdentifier(func)) = call.function() else {
                diagnostics.push(invalid_operand(range));
                return None;
            };
            if func.name_text() == "substitute" {
                return parse_substitute_hole(call, formals, diagnostics, range);
            }
            // An env-capture op (`parent.frame()`, `new.env(...)`). The
            // zero-arg rule for `parent.frame`/`globalenv`/`environment` lives
            // in `recognize_env_op`, so `parent.frame(2)` isn't recognized and
            // falls outside the set.
            let Some(op) = recognize_env_op(expr) else {
                diagnostics.push(invalid_operand(range));
                return None;
            };
            Some(RExpr::Env(op))
        },
        _ => {
            diagnostics.push(invalid_operand(range));
            None
        },
    }
}

/// `substitute(formal)` inside a hole: captures the formal unevaluated.
fn parse_substitute_hole(
    call: &RCall,
    formals: &[FormalDef],
    diagnostics: &mut Vec<DeclareDiagnostic>,
    range: TextRange,
) -> Option<RExpr> {
    let Some(AnyRExpression::RIdentifier(arg_ident)) = only_argument(call) else {
        diagnostics.push(invalid_operand(range));
        return None;
    };
    let name = arg_ident.name_text();
    let Some(idx) = formal_index(formals, &name) else {
        diagnostics.push(DeclareDiagnostic::new(
            DeclareDiagnosticKind::UnknownFormal { name },
            range,
        ));
        return None;
    };
    Some(RExpr::Substitute(ArgumentRef(idx)))
}

/// Recognize an env-capture op written as an R expression, or `None` when the
/// expression isn't one. Shared by `.()` holes (via [`parse_hole_rexpr`]) and
/// formal defaults (via [`static_default`]), so both read the same set.
///
/// The zero-argument ops (`parent.frame`, `globalenv`, `environment` and their
/// rlang aliases) must be called with no arguments. `parent.frame(2)` carries a
/// frame selector we don't interpret, so it falls outside the set. `new.env`
/// reads its `parent =` argument; a bare `new.env()` is `parent =
/// environment()`.
fn recognize_env_op(expr: &AnyRExpression) -> Option<EnvOp> {
    let AnyRExpression::RCall(call) = expr else {
        return None;
    };
    let Ok(AnyRExpression::RIdentifier(func)) = call.function() else {
        return None;
    };
    match func.name_text().as_str() {
        "parent.frame" | "caller_env" => zero_arg_env_op(call, EnvOp::ParentFrame),
        "globalenv" | "global_env" => zero_arg_env_op(call, EnvOp::GlobalEnv),
        "environment" | "current_env" => zero_arg_env_op(call, EnvOp::Environment),
        "new.env" => Some(EnvOp::NewEnv {
            parent: new_env_parent(call),
        }),
        _ => None,
    }
}

/// A zero-argument env op. Any argument (`parent.frame(2)`) means a frame
/// selector we don't interpret, so it isn't recognized.
fn zero_arg_env_op(call: &RCall, op: EnvOp) -> Option<EnvOp> {
    let has_arguments = call
        .arguments()
        .ok()
        .is_some_and(|args| args.items().iter().count() > 0);
    if has_arguments {
        return None;
    }
    Some(op)
}

/// Read `new.env`'s `parent =` argument as an [`EnvParent`], defaulting to
/// `Environment` for a bare `new.env()`. Lenient by design: an out-of-set
/// parent is `Unknown`, not a diagnostic, since the first cut collapses every
/// fresh env to `Nested` and the parent is only informational. `new.env`'s
/// other arguments (`hash`, `size`) are ignored.
fn new_env_parent(call: &RCall) -> EnvParent {
    let Some(parent) = named_argument(call, "parent") else {
        return EnvParent::Environment;
    };
    match recognize_env_op(&parent) {
        Some(EnvOp::ParentFrame) => EnvParent::ParentFrame,
        Some(EnvOp::Environment) => EnvParent::Environment,
        Some(EnvOp::GlobalEnv) => EnvParent::GlobalEnv,
        // A nested `new.env()` parent, or anything else, is out of the flat set.
        _ => EnvParent::Unknown,
    }
}

/// The value of `call`'s argument named `name`, or `None` when it has none.
fn named_argument(call: &RCall, name: &str) -> Option<AnyRExpression> {
    let args = call.arguments().ok()?;
    for item in args.items().iter() {
        let Ok(arg) = item else { continue };
        if argument_name(&arg).as_deref() == Some(name) {
            return arg.value();
        }
    }
    None
}

/// An `Eval` operand on a formal that an arg-centric entry declares
/// `Quote`/`Nse` is a contradiction: the operand forces what the entry says
/// is captured. Runs after every entry is parsed, since the two halves of
/// the contradiction can appear in either order in the source.
fn check_contradictions(
    env: &[(EnvironmentEffect, TextRange)],
    arguments: &[ArgumentEffect],
    formals: &[FormalDef],
    diagnostics: &mut Vec<DeclareDiagnostic>,
) {
    for (effect, range) in env {
        let mut refs = Vec::new();
        match effect {
            EnvironmentEffect::Attach { package } => collect_eval_refs(package, &mut refs),
            EnvironmentEffect::Source { path, envir } => {
                collect_eval_refs(path, &mut refs);
                collect_eval_refs(envir, &mut refs);
            },
        }

        for ArgumentRef(idx) in refs {
            let Some(arg_effect) = arguments.iter().find(|effect| effect.arg.0 == idx) else {
                continue;
            };
            if !matches!(arg_effect.mode, EvalMode::Quote | EvalMode::Nse { .. }) {
                continue;
            }
            let name = formals
                .get(idx)
                .map(|formal| formal.name.clone())
                .unwrap_or_default();
            diagnostics.push(DeclareDiagnostic::new(
                DeclareDiagnosticKind::ContradictoryLiveness { name },
                *range,
            ));
        }
    }
}

fn collect_eval_refs(expr: &DeclExpr, refs: &mut Vec<ArgumentRef>) {
    match expr {
        DeclExpr::Hole(rexpr) => collect_eval_ref(rexpr, refs),
        DeclExpr::If { cond, then, els } => {
            collect_eval_ref(cond, refs);
            collect_eval_refs(then, refs);
            collect_eval_refs(els, refs);
        },
    }
}

fn collect_eval_ref(rexpr: &RExpr, refs: &mut Vec<ArgumentRef>) {
    if let RExpr::Eval(arg_ref) = rexpr {
        refs.push(*arg_ref);
    }
}

#[cfg(test)]
mod tests {
    use aether_parser::parse;
    use aether_parser::RParserOptions;
    use biome_rowan::WalkEvent;

    use super::*;
    use crate::effects::declaration::resolve;
    use crate::effects::ResolvedArgumentEffect;
    use crate::semantic_index::NseScope;

    /// Parse `declare_source`, resolve it against `call_source`, and return the
    /// resolved effect at the first call argument.
    fn resolve_first_arg(
        declare_source: &str,
        call_source: &str,
    ) -> Option<ResolvedArgumentEffect> {
        let parsed = parse_declare(declare_source);
        assert!(parsed.diagnostics.is_empty());
        let call = first_call(call_source);
        let effects = resolve(&parsed.declaration, &call, &CallContext::new()).unwrap();
        effects.arguments.unwrap()[0].clone()
    }

    /// Parse `source` and return its first function definition.
    fn first_function(source: &str) -> RFunctionDefinition {
        let parsed = parse(source, RParserOptions::default());
        assert!(!parsed.has_error());
        parsed
            .tree()
            .syntax()
            .preorder()
            .find_map(|event| match event {
                WalkEvent::Enter(node) => RFunctionDefinition::cast(node),
                WalkEvent::Leave(_) => None,
            })
            .unwrap()
    }

    /// Parse `source` and return its first call.
    fn first_call(source: &str) -> RCall {
        let parsed = parse(source, RParserOptions::default());
        assert!(!parsed.has_error());
        parsed
            .tree()
            .syntax()
            .preorder()
            .find_map(|event| match event {
                WalkEvent::Enter(node) => RCall::cast(node),
                WalkEvent::Leave(_) => None,
            })
            .unwrap()
    }

    /// Parse `source`'s first function definition's `declare()` directive.
    fn parse_declare(source: &str) -> ParsedDeclaration {
        let function = first_function(source);
        parse_declaration(&function).unwrap()
    }

    #[test]
    fn braced_directive_is_found() {
        let parsed = parse_declare("function(x) { declare(x = Quote) }");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert_eq!(parsed.declaration.arguments[0].arg, ArgumentRef(0));
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
        assert!(parsed.diagnostics.is_empty());
    }

    #[test]
    fn unbraced_directive_is_found() {
        let parsed = parse_declare("function(x) declare(x = Quote)");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
    }

    #[test]
    fn tilde_declare_is_found() {
        let parsed = parse_declare("function(x) ~declare(x = Quote)");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
    }

    #[test]
    fn multiple_entries_resolve_exact_modes() {
        // `x = Quote` and `y = Nse(.(parent.frame()), eager = FALSE)`. Both
        // entries parse; resolving against a call reads `y`'s scope operand as
        // the call-site scope, lazily.
        let parsed = parse_declare(
            "function(x, y) declare(x = Quote, y = Nse(.(parent.frame()), eager = FALSE))",
        );
        assert_eq!(parsed.declaration.arguments.len(), 2);
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
        assert!(parsed.diagnostics.is_empty());

        let call = first_call("f(a, b)");
        let effects = resolve(&parsed.declaration, &call, &CallContext::new()).unwrap();
        let arguments = effects.arguments.unwrap();
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Quote { .. })
        ));
        assert!(matches!(
            arguments[1],
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Lazy,
            })
        ));
    }

    #[test]
    fn bare_quote_and_call_quote_are_equivalent() {
        let bare = parse_declare("function(x) declare(x = Quote)");
        let call = parse_declare("function(x) declare(x = Quote())");
        assert!(matches!(
            bare.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
        assert!(matches!(
            call.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
    }

    #[test]
    fn bare_nse_defaults_to_nested_eager() {
        // Bare `Nse` synthesizes `new.env(parent = parent.frame())`, resolving
        // to a fresh nested scope, eagerly.
        assert!(matches!(
            resolve_first_arg("function(x) declare(x = Nse)", "f(y)"),
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Nested,
                timing: NseTiming::Eager,
            })
        ));
    }

    #[test]
    fn nse_scope_operand_parent_frame() {
        // `Nse(.(parent.frame()))` names the call-site scope: `Current`, eager.
        assert!(matches!(
            resolve_first_arg("function(x) declare(x = Nse(.(parent.frame())))", "f(y)"),
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Eager,
            })
        ));
    }

    #[test]
    fn nse_scope_operand_reads_formal() {
        // `Nse(.(envir))` reads `envir`'s env-op default. Absent in the call, it
        // falls back to `parent.frame()` -> `Current`.
        assert!(matches!(
            resolve_first_arg(
                "function(x, envir = parent.frame()) declare(x = Nse(.(envir)))",
                "f(y)"
            ),
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Eager,
            })
        ));
    }

    #[test]
    fn nse_named_eager_only() {
        // A bare scope (absent, so the `new.env()` default -> Nested) with
        // `eager = FALSE`: nested, lazy.
        assert!(matches!(
            resolve_first_arg("function(x) declare(x = Nse(eager = FALSE))", "f(y)"),
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Nested,
                timing: NseTiming::Lazy,
            })
        ));
    }

    #[test]
    fn nse_invalid_scope_operand_drops_entry() {
        // A scope operand that isn't a `.()` hole (a bare string) falls outside
        // the interpreted set, so the entry drops with `InvalidOperand`.
        let parsed = parse_declare("function(x) declare(x = Nse(\"foo\"))");
        assert!(parsed.declaration.arguments.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidOperand
        ));
    }

    #[test]
    fn nse_invalid_scope_operand_partial_entry_survives() {
        // `Nse(bar)`'s scope isn't a `.()` hole, so the `x` entry drops; `y`'s
        // plain `Quote` still parses.
        let parsed = parse_declare("function(x, y) declare(x = Nse(bar), y = Quote)");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert_eq!(parsed.declaration.arguments[0].arg, ArgumentRef(1));
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidOperand
        ));
    }

    #[test]
    fn evalq_stub_round_trips_through_resolve() {
        // `evalq(foo)`: `envir` absent, reads its `parent.frame()` default ->
        // Current, eager. `evalq(foo, e)`: explicit `envir` isn't interpreted,
        // so the scope drops and the Nse degrades to a suppressed Quote.
        let declare =
            "function(expr, envir = parent.frame(), enclos) declare(expr = Nse(.(envir)))";
        assert!(matches!(
            resolve_first_arg(declare, "evalq(foo)"),
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Eager,
            })
        ));
        assert!(matches!(
            resolve_first_arg(declare, "evalq(foo, e)"),
            Some(ResolvedArgumentEffect::Quote { .. })
        ));
    }

    #[test]
    fn local_stub_round_trips_through_resolve() {
        // `local(x)`: `envir` absent, reads its `new.env()` default -> Nested,
        // eager. `local(x, e)`: explicit `envir` drops the scope -> Quote.
        let declare = "function(expr, envir = new.env()) declare(expr = Nse(.(envir)))";
        assert!(matches!(
            resolve_first_arg(declare, "local(x)"),
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Nested,
                timing: NseTiming::Eager,
            })
        ));
        assert!(matches!(
            resolve_first_arg(declare, "local(x, e)"),
            Some(ResolvedArgumentEffect::Quote { .. })
        ));
    }

    #[test]
    fn nse_scope_reading_formal_is_not_contradiction() {
        // `.(envir)` in an `Nse` scope is a live read of `envir`, not a capture,
        // so it doesn't raise `ContradictoryLiveness`.
        let parsed =
            parse_declare("function(expr, envir = parent.frame()) declare(expr = Nse(.(envir)))");
        assert!(parsed.diagnostics.is_empty());
    }

    #[test]
    fn nse_scope_operand_new_env_captures_parent() {
        // `Nse(.(new.env(parent = globalenv())))` parses its parent, but the
        // first cut resolves any fresh env to `Nested`.
        assert!(matches!(
            resolve_first_arg(
                "function(x) declare(x = Nse(.(new.env(parent = globalenv()))))",
                "f(y)"
            ),
            Some(ResolvedArgumentEffect::Nse {
                scope: NseScope::Nested,
                timing: NseTiming::Eager,
            })
        ));
    }

    #[test]
    fn nse_invalid_eager_value() {
        let parsed = parse_declare("function(x) declare(x = Nse(eager = 1))");
        assert!(parsed.declaration.arguments.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidNseArgument { ref name } if name == "eager"
        ));
    }

    #[test]
    fn unknown_formal_is_a_diagnostic() {
        let parsed = parse_declare("function(x) declare(z = Quote)");
        assert!(parsed.declaration.arguments.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::UnknownFormal { ref name } if name == "z"
        ));
    }

    #[test]
    fn attach_substitute_operand() {
        let parsed = parse_declare("function(pkg) declare(Attach(.(substitute(pkg))))");
        assert_eq!(parsed.declaration.env.len(), 1);
        assert!(matches!(
            parsed.declaration.env[0],
            EnvironmentEffect::Attach {
                package: DeclExpr::Hole(RExpr::Substitute(ArgumentRef(0)))
            }
        ));
        assert!(parsed.diagnostics.is_empty());
    }

    #[test]
    fn attach_if_else_shape() {
        let parsed = parse_declare(
            "function(character.only, pkg) declare(Attach(if (.(character.only)) .(pkg) else .(substitute(pkg))))",
        );
        let EnvironmentEffect::Attach { package } = &parsed.declaration.env[0] else {
            panic!("expected Attach");
        };
        let DeclExpr::If { cond, then, els } = package else {
            panic!("expected If");
        };
        assert!(matches!(cond, RExpr::Eval(ArgumentRef(0))));
        assert!(matches!(
            **then,
            DeclExpr::Hole(RExpr::Eval(ArgumentRef(1)))
        ));
        assert!(matches!(
            **els,
            DeclExpr::Hole(RExpr::Substitute(ArgumentRef(1)))
        ));
        assert!(parsed.diagnostics.is_empty());
    }

    #[test]
    fn source_envir_if_else_shape() {
        let parsed = parse_declare(
            "function(file, local) declare(Source(.(file), envir = if (.(local)) .(parent.frame()) else .(globalenv())))",
        );
        let EnvironmentEffect::Source { path, envir } = &parsed.declaration.env[0] else {
            panic!("expected Source");
        };
        assert!(matches!(path, DeclExpr::Hole(RExpr::Eval(ArgumentRef(0)))));
        let DeclExpr::If { cond, then, els } = envir else {
            panic!("expected If");
        };
        assert!(matches!(cond, RExpr::Eval(ArgumentRef(1))));
        assert!(matches!(
            **then,
            DeclExpr::Hole(RExpr::Env(EnvOp::ParentFrame))
        ));
        assert!(matches!(
            **els,
            DeclExpr::Hole(RExpr::Env(EnvOp::GlobalEnv))
        ));
        assert!(parsed.diagnostics.is_empty());
    }

    #[test]
    fn source_missing_envir_is_invalid() {
        let parsed = parse_declare("function(file) declare(Source(.(file)))");
        assert!(parsed.declaration.env.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidOperand
        ));
    }

    #[test]
    fn env_op_with_argument_is_invalid() {
        // `parent.frame(2)` carries a frame selector we don't interpret, so the
        // hole falls outside the interpreted set.
        let parsed =
            parse_declare("function(file) declare(Source(.(file), envir = .(parent.frame(2))))");
        assert!(parsed.declaration.env.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidOperand
        ));
    }

    #[test]
    fn attach_without_hole_wrapper_is_invalid() {
        let parsed = parse_declare("function(pkg) declare(Attach(substitute(pkg)))");
        assert!(parsed.declaration.env.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidOperand
        ));
    }

    #[test]
    fn attach_unknown_formal_in_hole() {
        let parsed = parse_declare("function(pkg) declare(Attach(.(unknown)))");
        assert!(parsed.declaration.env.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::UnknownFormal { ref name } if name == "unknown"
        ));
    }

    #[test]
    fn if_without_else_is_invalid() {
        let parsed = parse_declare(
            "function(character.only, pkg) declare(Attach(if (.(character.only)) .(pkg)))",
        );
        assert!(parsed.declaration.env.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::MissingElse
        ));
    }

    #[test]
    fn formal_named_attach_is_arg_centric() {
        // The named argument `Attach = Quote` is structurally an arg-centric
        // entry, not the `Attach` effect, even though the formal is named
        // the same as the effect keyword.
        let parsed = parse_declare("function(Attach) declare(Attach = Quote)");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert_eq!(parsed.declaration.arguments[0].arg, ArgumentRef(0));
        assert!(parsed.declaration.env.is_empty());
    }

    #[test]
    fn duplicate_entry_keeps_first() {
        let parsed = parse_declare("function(x) declare(x = Quote, x = Nse)");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::DuplicateEntry { ref name } if name == "x"
        ));
    }

    #[test]
    fn contradiction_between_quote_and_eval_operand() {
        let parsed = parse_declare("function(x) declare(x = Quote, Attach(.(x)))");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert_eq!(parsed.declaration.env.len(), 1);
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::ContradictoryLiveness { ref name } if name == "x"
        ));
    }

    #[test]
    fn no_directive_returns_none() {
        let function = first_function("function(x) { x + 1 }");
        assert!(parse_declaration(&function).is_none());
    }

    #[test]
    fn comment_before_declare_is_still_found() {
        let parsed = parse_declare("function(x) {\n  # comment\n  declare(x = Quote)\n}");
        assert_eq!(parsed.declaration.arguments.len(), 1);
    }

    #[test]
    fn bool_default_is_static() {
        let function = first_function("function(x, flag = FALSE) NULL");
        let formals = build_formals(&function.parameters().unwrap());
        assert_eq!(formals[1].default, Some(StaticValue::Bool(false)));
    }

    #[test]
    fn non_static_default_is_not_captured() {
        let function = first_function("function(x, opt = getOption(\"foo\")) NULL");
        let formals = build_formals(&function.parameters().unwrap());
        assert_eq!(formals[1].default, None);
    }

    #[test]
    fn dots_formal_is_named_dots() {
        let function = first_function("function(x, ...) NULL");
        let formals = build_formals(&function.parameters().unwrap());
        assert_eq!(formals[1].name, "...");
    }

    #[test]
    fn library_stub_round_trips_through_resolve() {
        // The design doc's `library.ty.R` stub, parsed and then resolved
        // against calls the same way `EffectSource::Declared` would, to
        // check it reproduces the handcrafted `attach_declaration()` in
        // `declaration.rs`'s own tests.
        let function = first_function(
            "function(package, help, pos = 2, lib.loc = NULL, character.only = FALSE) declare(Attach(if (.(character.only)) .(package) else .(substitute(package))))",
        );
        let parsed = parse_declaration(&function).unwrap();
        assert!(parsed.diagnostics.is_empty());

        let call = first_call("library(dplyr)");
        let effects = resolve(&parsed.declaration, &call, &CallContext::new()).unwrap();
        assert_eq!(effects.attach.as_deref(), Some("dplyr"));
        let arguments = effects.arguments.unwrap();
        assert!(matches!(
            arguments[0],
            Some(ResolvedArgumentEffect::Quote { .. })
        ));

        let call = first_call("library(\"x\", character.only = TRUE)");
        let effects = resolve(&parsed.declaration, &call, &CallContext::new()).unwrap();
        assert_eq!(effects.attach.as_deref(), Some("x"));
    }
}
