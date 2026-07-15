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
use crate::effects::EnvironmentEffect;
use crate::effects::EvalMode;
use crate::effects::FormalDef;
use crate::effects::RExpr;
use crate::effects::StaticValue;
use crate::semantic_index::NseScope;
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
    /// `Nse`'s `scope` or `eager` argument isn't one of its allowed
    /// literals (`scope` takes no partial matching, `eager` a bare bool).
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
/// included. A `TRUE`/`FALSE` default becomes a [`StaticValue`]; anything
/// else (a call like `getOption(...)`, or no default) leaves it `None`.
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
                .and_then(|value| CallContext::new().resolve_static_bool(&value))
                .map(StaticValue::Bool);
            FormalDef { name, default }
        })
        .collect()
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
    let Some(mode) = parse_eval_mode(&value, diagnostics) else {
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

/// `Source(<declexpr>)` or `Source(<declexpr>, guard = <hole>)`. `guard`
/// must be a plain `.()` hole (an [`RExpr`]), not a full [`DeclExpr`]: it's
/// consulted only as a bool bail, never as an operand `path` folds through.
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
            name: "guard".to_string(),
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

    let guard = match bound_value(&matched, &values, 1) {
        None => None,
        Some(guard_operand) => {
            let guard_range = guard_operand.syntax().text_trimmed_range();
            let Some(hole) = hole_call_argument(guard_operand) else {
                diagnostics.push(invalid_operand(guard_range));
                return;
            };
            let Some(rexpr) = parse_hole_rexpr(&hole, formals, diagnostics, guard_range) else {
                return;
            };
            Some(rexpr)
        },
    };

    env.push((EnvironmentEffect::Source { path, guard }, range));
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

/// Parse an arg-centric RHS: `Quote`, `Quote()`, `Nse`, or `Nse(...)`.
fn parse_eval_mode(
    value: &AnyRExpression,
    diagnostics: &mut Vec<DeclareDiagnostic>,
) -> Option<EvalMode> {
    let range = value.syntax().text_trimmed_range();
    match value {
        // Bare is an alias for a no-arg call: `Quote` == `Quote()`, `Nse` ==
        // `Nse()`.
        AnyRExpression::RIdentifier(ident) => match ident.name_text().as_str() {
            "Quote" => Some(EvalMode::Quote),
            "Nse" => Some(EvalMode::Nse {
                scope: NseScope::Nested,
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
                "Nse" => parse_nse_call(call, range, diagnostics),
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

/// `Nse(scope = c("nested", "current"), eager = TRUE)`, matched with
/// [`match_signature`] against that synthetic signature. That's the reuse
/// this vocabulary is designed for: parsing `Nse("current", eager = FALSE)`
/// is the same positional/named resolution as any other call, not a
/// special case.
fn parse_nse_call(
    call: &RCall,
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
        None => NseScope::Nested,
        Some(expr) => {
            let Some(scope) = resolve_scope_literal(expr) else {
                diagnostics.push(DeclareDiagnostic::new(
                    DeclareDiagnosticKind::InvalidNseArgument {
                        name: "scope".to_string(),
                    },
                    range,
                ));
                return None;
            };
            scope
        },
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

/// `scope`'s value, exactly `"nested"` or `"current"`. No partial matching:
/// `"n"` isn't `"nested"`.
fn resolve_scope_literal(expr: &AnyRExpression) -> Option<NseScope> {
    match CallContext::new().resolve_static_string(expr)?.as_str() {
        "nested" => Some(NseScope::Nested),
        "current" => Some(NseScope::Current),
        _ => None,
    }
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
            if func.name_text() != "substitute" {
                diagnostics.push(invalid_operand(range));
                return None;
            }
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
        },
        _ => {
            diagnostics.push(invalid_operand(range));
            None
        },
    }
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
            EnvironmentEffect::Source { path, guard } => {
                collect_eval_refs(path, &mut refs);
                if let Some(guard) = guard {
                    collect_eval_ref(guard, &mut refs);
                }
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
        let parsed =
            parse_declare("function(x, y) declare(x = Quote, y = Nse(\"current\", eager = FALSE))");
        assert_eq!(parsed.declaration.arguments.len(), 2);
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Quote
        ));
        assert!(matches!(
            parsed.declaration.arguments[1].mode,
            EvalMode::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Lazy
            }
        ));
        assert!(parsed.diagnostics.is_empty());
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
        let parsed = parse_declare("function(x) declare(x = Nse)");
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Nse {
                scope: NseScope::Nested,
                timing: NseTiming::Eager
            }
        ));
    }

    #[test]
    fn nse_positional_scope() {
        let parsed = parse_declare("function(x) declare(x = Nse(\"current\"))");
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Nse {
                scope: NseScope::Current,
                timing: NseTiming::Eager
            }
        ));
    }

    #[test]
    fn nse_named_eager_only() {
        let parsed = parse_declare("function(x) declare(x = Nse(eager = FALSE))");
        assert!(matches!(
            parsed.declaration.arguments[0].mode,
            EvalMode::Nse {
                scope: NseScope::Nested,
                timing: NseTiming::Lazy
            }
        ));
    }

    #[test]
    fn nse_scope_no_partial_matching() {
        // `"n"` isn't `"nested"`: no partial matching, so the `x` entry
        // drops but `y`'s plain `Quote` survives.
        let parsed = parse_declare("function(x, y) declare(x = Nse(\"n\"), y = Quote)");
        assert_eq!(parsed.declaration.arguments.len(), 1);
        assert_eq!(parsed.declaration.arguments[0].arg, ArgumentRef(1));
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidNseArgument { ref name } if name == "scope"
        ));
    }

    #[test]
    fn nse_invalid_scope_value() {
        let parsed = parse_declare("function(x) declare(x = Nse(\"foo\"))");
        assert!(parsed.declaration.arguments.is_empty());
        assert_eq!(parsed.diagnostics.len(), 1);
        assert!(matches!(
            parsed.diagnostics[0].kind,
            DeclareDiagnosticKind::InvalidNseArgument { ref name } if name == "scope"
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
    fn source_with_guard() {
        let parsed =
            parse_declare("function(file, local) declare(Source(.(file), guard = .(local)))");
        let EnvironmentEffect::Source { path, guard } = &parsed.declaration.env[0] else {
            panic!("expected Source");
        };
        assert!(matches!(path, DeclExpr::Hole(RExpr::Eval(ArgumentRef(0)))));
        assert!(matches!(guard, Some(RExpr::Eval(ArgumentRef(1)))));
        assert!(parsed.diagnostics.is_empty());
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
        // The design doc's `library.Rty` stub, parsed and then resolved
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
