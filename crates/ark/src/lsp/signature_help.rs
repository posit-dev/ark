//
// signature_help.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::call::RArgument;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::*;
use harp::r_null;
use harp::syntax::sym_quote_invalid;
use harp::utils::r_formals;
use harp::utils::r_is_function;
use harp::utils::r_is_object;
use harp::utils::r_type2char;
use harp::utils::r_typeof;
use harp::RSymbol;
use libr::*;
use log::info;
use oak_db::Db;
use oak_db::File;
use oak_db::Formal;
use tower_lsp_server::ls_types::Documentation;
use tower_lsp_server::ls_types::ParameterInformation;
use tower_lsp_server::ls_types::ParameterLabel;
use tower_lsp_server::ls_types::SignatureHelp;
use tower_lsp_server::ls_types::SignatureInformation;
use tree_sitter::Node;

use crate::console;
use crate::lsp::call_arguments::active_parameter;
use crate::lsp::call_arguments::call_at;
use crate::lsp::call_arguments::describe_callee;
use crate::lsp::call_arguments::CallArguments;
use crate::lsp::call_arguments::CallAt;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::help::RHtmlHelp;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeTypeExt;

/// Signature help from the static pass, which runs on the main loop without R.
pub(crate) struct StaticSignatureHelp {
    /// Built from the callee's static description, without parameter
    /// documentation. `None` when oak can't describe the callee.
    pub(crate) help: Option<SignatureHelp>,
    pub(crate) request: SignatureHelpRequest,
}

/// Input for [r_signature_help()]. Owned plain data with no database handle,
/// so an R error escaping the session pass can't leak one.
#[derive(Debug)]
pub(crate) struct SignatureHelpRequest {
    arguments: CallArguments,
    /// Help page for the parameter documentation. `None` when the callee
    /// text names no topic.
    topic: Option<HelpTopic>,
}

#[derive(Debug, Clone)]
struct HelpTopic {
    /// Function name, `f` in `pkg::f`.
    name: String,
    /// Package of `pkg::f`, `None` for a bare name.
    package: Option<String>,
}

/// A session reply replaces static help when the callee evaluates to a
/// function. Otherwise the static help stands.
#[derive(Debug)]
pub(crate) enum SignatureHelpReply {
    /// The callee can't be evaluated or isn't a function in the session, so
    /// the static help stands.
    Decline,
    /// Replaces static help with evaluated formals and parameter docs.
    Fill(SignatureHelp),
}

impl SignatureHelpReply {
    /// Returns `fallback` for [SignatureHelpReply::Decline], otherwise the
    /// reply's help.
    pub(crate) fn help_or(self, fallback: Option<SignatureHelp>) -> Option<SignatureHelp> {
        match self {
            SignatureHelpReply::Decline => fallback,
            SignatureHelpReply::Fill(help) => Some(help),
        }
    }
}

/// Recognizes the call around the cursor, answers from the callee's static
/// description, and builds the request for [r_signature_help()]. `None`
/// outside a call's parentheses.
///
/// Doesn't call R, so it can run while R is busy.
pub(crate) fn static_signature_help(
    context: &DocumentContext,
    db: &dyn Db,
    file: File,
) -> anyhow::Result<Option<StaticSignatureHelp>> {
    let Some(CallAt { callee, arguments }) = call_at(context)? else {
        return Ok(None);
    };

    let help = describe_callee(db, file, &callee)?.map(|description| {
        let parameters = description
            .formals
            .iter()
            .map(|formal| SignatureParameter {
                name: formal.name.clone(),
                label: static_argument_label(formal),
                documentation: None,
            })
            .collect();
        signature_help(&arguments, parameters)
    });

    let topic = help_topic(&callee, context.contents);
    let request = SignatureHelpRequest { arguments, topic };

    Ok(Some(StaticSignatureHelp { help, request }))
}

/// `None` when the callee text doesn't name a topic, such as an incomplete
/// `utils::`. The session pass then answers without parameter documentation
/// rather than discarding the static help.
fn help_topic(callee: &Node, contents: &str) -> Option<HelpTopic> {
    if !callee.is_namespace_operator() {
        let name = callee.node_to_string(contents).ok()?;
        return Some(HelpTopic {
            name,
            package: None,
        });
    }

    let package = callee.child_by_field_name("lhs")?;
    let package = package.node_to_string(contents).ok()?;

    let name = callee.child_by_field_name("rhs")?;
    let name = name.node_to_string(contents).ok()?;

    Some(HelpTopic {
        name,
        package: Some(package),
    })
}

// Flatten multiline defaults for display without evaluating or deparsing them.
// This is a source-text approximation, not an R-normalized expression.
fn static_argument_label(formal: &Formal) -> String {
    let Some(default) = &formal.default else {
        return argument_label_truncate(formal.name.clone());
    };

    let default: Vec<&str> = default
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    argument_label_truncate(format!("{} = {}", formal.name, default.join(" ")))
}

/// Signature help from the function that the request's callee would call,
/// with parameter documentation from its help page. Declines when the session
/// has no such function, so the static help stands.
///
/// SAFETY: Requires access to the R runtime.
pub(crate) fn r_signature_help(
    request: &SignatureHelpRequest,
) -> anyhow::Result<SignatureHelpReply> {
    let SignatureHelpRequest { arguments, topic } = request;

    let Some(object) = r_callee_function(&arguments.callee) else {
        return Ok(SignatureHelpReply::Decline);
    };
    let formals = r_formals(*object)?;

    let help = match topic {
        Some(topic) => RHtmlHelp::from_function(topic.name.as_str(), topic.package.as_deref()),
        None => Ok(None),
    };

    let parameters = formals
        .iter()
        .map(|argument| {
            let mut documentation = None;
            if let Ok(Some(ref help)) = help {
                let markup = help.parameter(&argument.name);
                if let Ok(Some(markup)) = markup {
                    documentation = Some(Documentation::MarkupContent(markup));
                }
            }

            SignatureParameter {
                name: argument.name.clone(),
                label: argument_label(argument.name.clone(), argument.value.sexp),
                documentation,
            }
        })
        .collect();

    let help = signature_help(arguments, parameters);

    info!("{:?}", help);
    Ok(SignatureHelpReply::Fill(help))
}

struct SignatureParameter {
    name: String,
    label: String,
    documentation: Option<Documentation>,
}

// Both passes use the same label offsets and argument-matching rules.
fn signature_help(arguments: &CallArguments, parameters: Vec<SignatureParameter>) -> SignatureHelp {
    // The signature label. We generate this as we walk through the
    // parameters, so we can more easily record offsets.
    let mut label = String::new();
    label.push_str(arguments.callee.as_str());
    label.push('(');

    let mut names = Vec::with_capacity(parameters.len());
    let mut information = Vec::with_capacity(parameters.len());

    for (index, parameter) in parameters.into_iter().enumerate() {
        if index > 0 {
            label.push_str(", ");
        }

        // Compute signature offsets.
        let start = label.len() as u32;
        let end = start + parameter.label.len() as u32;

        // Add the argument label to the overall label.
        label.push_str(parameter.label.as_str());

        names.push(parameter.name);
        information.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation: parameter.documentation,
        });
    }

    // Add a closing parenthesis.
    label.push(')');

    // NOTE: It seems like the frontend still tries to highlight the first
    // parameter when the offset is set to none, so here we just force it to
    // match no available argument.
    let offset = active_parameter(arguments, &names).unwrap_or(names.len() + 1);
    let offset = Some(u32::try_from(offset).unwrap_or_default());

    let signature = SignatureInformation {
        label,
        documentation: None,
        parameters: Some(information),
        active_parameter: offset,
    };

    SignatureHelp {
        signatures: vec![signature],
        active_signature: None,
        active_parameter: offset,
    }
}

/// Formals of the function that a call to `callee` would invoke, or `None` if
/// the session has no such function.
///
/// SAFETY: Requires access to the R runtime.
pub(crate) fn r_callee_formals(callee: &str) -> anyhow::Result<Option<Vec<RArgument>>> {
    let Some(object) = r_callee_function(callee) else {
        return Ok(None);
    };

    Ok(Some(r_formals(*object)?))
}

/// The function that a call to `callee` would invoke in
/// `console::selected_env()`. `None` if the text contains `(`, fails to parse
/// or evaluate, or doesn't give a function.
///
/// A bare name is looked up the way R looks up the function of a call, which
/// skips non-function bindings: after `t <- 5`, `t(m)` still calls
/// `base::t()`. Other callees, such as `pkg::f` or `obj$f`, are evaluated as
/// values. The call slot, custom completions and signature help all go
/// through here so they agree on which function a call names.
///
/// SAFETY: Requires access to the R runtime.
pub(crate) fn r_callee_function(callee: &str) -> Option<RObject> {
    let env = console::selected_env();

    let object = match callee_name(callee) {
        Some(name) => RFunction::new("base", "get0")
            .add(name)
            .param("envir", env)
            .param("mode", "function")
            .call(),
        None => harp::parse_eval(callee, RParseEvalOptions {
            forbid_function_calls: true,
            env,
        }),
    };

    // The guard rejects callee text containing `(`, and unfinished code can
    // name an undefined callee. The static answer stands in both cases, so
    // the error isn't reported.
    let object = object.ok()?;

    // Not uncommon for tree-sitter to detect partially written code as a
    // call, like:
    // ---
    // mtcars$
    // plot(1:5)
    // ---
    // Where it detects `mtcars$plot` as the LHS of the call.
    // That is actually how R would parse this, but the user might be writing
    // `mtcars$` and requesting completions for the `$` when this occurs.
    // A non-function has no formals to match against.
    if !r_is_function(*object) {
        return None;
    }

    Some(object)
}

/// The name of a callee that is a bare symbol, like `t` or `` `my fun` ``.
fn callee_name(callee: &str) -> Option<String> {
    let expr = harp::parse_expr(callee).ok()?;
    if r_typeof(*expr) != SYMSXP {
        return None;
    }
    Some(String::from(RSymbol::new(*expr).ok()?))
}

fn argument_label(name: String, value: SEXP) -> String {
    // Specially handle `R_MissingArg`, which looks like a `SYMSXP`,
    // but we don't want to add `=` to it. This is what we see when
    // there is no default argument (and also for `...`).
    if value == harp::missing() {
        return argument_label_truncate(name);
    }

    let out = name + " = " + obj_label(value).as_str();

    argument_label_truncate(out)
}

fn argument_label_truncate(x: String) -> String {
    // Max of 200 Unicode Scalar Values seems reasonable to prevent any argument from
    // overwhelming the pop up
    const MAX_CHARS: usize = 200;

    // Safer than `x.truncate()` which would panic if we are between `char` boundaries
    match x.char_indices().nth(MAX_CHARS) {
        None => x,
        Some((idx, _)) => {
            let mut x = x[..idx].to_string();
            x.push_str("...");
            x
        },
    }
}

fn obj_label(x: SEXP) -> String {
    if r_is_object(x) {
        // Just showing the class name for inlined complex objects
        return s3_label(x);
    }
    if r_dim(x) != r_null() {
        // In the odd case of an inlined array, just show type name
        return type_label(x);
    }

    // Most cases are either:
    // - `NULL`
    // - Call
    // - Symbol
    // - Scalar objects that get inlined automatically
    // However, it is possible to have more complex arguments inlined into the function
    // signature, like vctrs does with `fn_inline_formals()`. We also handle most of
    // those cases well.
    match r_typeof(x) {
        NILSXP => null_label(),
        LANGSXP => call_label(x),
        SYMSXP => sym_label(x),
        LGLSXP => vec_label(x, lgl_to_pretty_string),
        INTSXP => vec_label(x, int_to_pretty_string),
        REALSXP => vec_label(x, dbl_to_pretty_string),
        CPLXSXP => vec_label(x, cpl_to_pretty_string),
        STRSXP => vec_label(x, chr_to_pretty_string),
        VECSXP => list_label(),
        _ => type_label(x),
    }
}

fn type_label(x: SEXP) -> String {
    let out = r_type2char(r_typeof(x));
    let out = String::from("<") + out.as_str() + ">";
    out
}

fn s3_label(x: SEXP) -> String {
    match harp::format::s3_class_to_string(x) {
        Ok(class) => class,
        Err(err) => {
            log::error!("{err:?}");
            String::from("<???>")
        },
    }
}

fn null_label() -> String {
    harp::format::null_to_string()
}

fn list_label() -> String {
    // We probably don't want to do anything more here. An inlined list should be rare
    // and might come from something massive like a data frame or model object.
    String::from("<list>")
}

fn vec_label(x: SEXP, elt_to_pretty_string: fn(SEXP, isize) -> String) -> String {
    let size = r_length(x);
    let scalar = size == 1;

    const TRUNCATE_SIZE: isize = 10;
    let truncate = size > TRUNCATE_SIZE;

    // Cap `size` if needed
    let size = if truncate { TRUNCATE_SIZE } else { size };

    let mut out = String::from("");

    if !scalar {
        out.push_str("c(");
    }

    for i in 0..size {
        let elt = elt_to_pretty_string(x, i);
        out.push_str(elt.as_str());

        if i != size - 1 {
            out.push_str(", ");
        }
    }

    if truncate {
        out.push_str(", ...");
    }

    if !scalar {
        out.push(')');
    }

    out
}

fn lgl_to_pretty_string(x: SEXP, i: isize) -> String {
    harp::format::lgl_to_string(r_lgl_get(x, i))
}
fn int_to_pretty_string(x: SEXP, i: isize) -> String {
    harp::format::int_to_string(r_int_get(x, i))
}
fn dbl_to_pretty_string(x: SEXP, i: isize) -> String {
    harp::format::dbl_to_string(r_dbl_get(x, i))
}
fn cpl_to_pretty_string(x: SEXP, i: isize) -> String {
    harp::format::cpl_to_string(r_cpl_get(x, i))
}
fn chr_to_pretty_string(x: SEXP, i: isize) -> String {
    harp::format::str_to_string(r_chr_get(x, i))
}

fn sym_label(x: SEXP) -> String {
    if x == harp::missing() {
        panic!("`R_MissingArg` should have been handled earlier.");
    }

    let x = RSymbol::new_unchecked(x);
    let x = String::from(x);
    let x = sym_quote_invalid(x.as_str());

    x
}

fn call_label(x: SEXP) -> String {
    match harp::call::expr_deparse_collapse(x) {
        Ok(x) => x,
        Err(err) => {
            log::error!("Can't convert call to text: {err:?}.");
            String::from("<call>")
        },
    }
}

#[cfg(test)]
mod tests {
    use harp::call::RCall;
    use harp::object::*;
    use harp::r_char;
    use harp::r_null;
    use harp::r_symbol;
    use harp::RObject;
    use libr::R_xlen_t;
    use tower_lsp_server::ls_types::ParameterLabel;
    use tower_lsp_server::ls_types::SignatureHelp;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::document_context::TestDocument;
    use crate::lsp::signature_help::argument_label;
    use crate::lsp::signature_help::r_signature_help;
    use crate::lsp::signature_help::static_signature_help;
    use crate::lsp::signature_help::SignatureHelpReply;
    use crate::lsp::signature_help::StaticSignatureHelp;
    use crate::panic;

    /// Runs only the static pass, as when R is busy.
    fn static_signature_help_at_cursor(code: &str) -> Option<StaticSignatureHelp> {
        let _boundary = panic::test_boundary();
        let (text, point) = point_from_cursor(code);
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        static_signature_help(&context, doc.state().db(), doc.file()).unwrap()
    }

    /// Runs both passes, as when R is idle. Requires the R thread.
    fn signature_help_at_cursor(code: &str) -> Option<SignatureHelp> {
        let StaticSignatureHelp { help, request } = static_signature_help_at_cursor(code)?;
        r_signature_help(&request).unwrap().help_or(help)
    }

    fn label(help: &SignatureHelp) -> &str {
        help.signatures[0].label.as_str()
    }

    #[test]
    fn test_basic_signature_help() {
        crate::r_task(|| {
            let help = signature_help_at_cursor("library(@)").unwrap();
            assert_eq!(help.signatures.len(), 1);

            // Looking for the label offset into `library(package, ...etc)` for `package`
            let signature = help.signatures.first().unwrap();
            let label = &signature
                .parameters
                .as_ref()
                .unwrap()
                .first()
                .unwrap()
                .label;
            assert_eq!(label, &ParameterLabel::LabelOffsets([8, 15]));
        })
    }

    #[test]
    fn test_no_signature_help_outside_parentheses() {
        assert!(static_signature_help_at_cursor("library@()").is_none());
        assert!(static_signature_help_at_cursor("library()@").is_none());
    }

    #[test]
    fn test_signature_help_argument_defaults() {
        crate::r_task(|| {
            // Define function in global env
            let fun = r#"
fn <- function(
  x,
  ...,
  by = NULL,
  value = 2.5,
  string = "foo",
  keep = c("yes", "no", "maybe"),
  length = c(2L, 3L),
  call = identity(x),
  lst = list(1, 2)
) { }
"#;
            harp::parse_eval_global(fun).unwrap();

            let help = signature_help_at_cursor("fn(@)").unwrap();

            // Check expected signature label
            assert_eq!(
                label(&help),
                r#"fn(x, ..., by = NULL, value = 2.5, string = "foo", keep = c("yes", "no", "maybe"), length = c(2L, 3L), call = identity(x), lst = list(1, 2))"#
            );

            // Clean up
            harp::parse_eval_global("rm(fn)").unwrap();
        })
    }

    #[test]
    fn test_static_signature_help_argument_defaults() {
        let code = r#"
fn <- function(
  x,
  ...,
  by = NULL,
  value = 2.5,
  string = "foo",
  keep = c("yes", "no", "maybe"),
  length = c(2L, 3L),
  call = identity(x),
  lst = list(1, 2)
) { }
fn(@)
"#;
        let help = static_signature_help_at_cursor(code).unwrap().help.unwrap();
        assert_eq!(
            label(&help),
            r#"fn(x, ..., by = NULL, value = 2.5, string = "foo", keep = c("yes", "no", "maybe"), length = c(2L, 3L), call = identity(x), lst = list(1, 2))"#
        );
    }

    #[test]
    fn test_static_signature_help_defaults_are_source_text() {
        // Session labels normalize quotes and numbers. Static labels retain
        // their source spelling.
        let code = "fn <- function(chr = 'a', num = 1e3, f = function(x) {\n  x\n}) NULL\nfn(@)";
        let help = static_signature_help_at_cursor(code).unwrap().help.unwrap();
        assert_eq!(
            label(&help),
            "fn(chr = 'a', num = 1e3, f = function(x) { x })"
        );

        let parameters = help.signatures[0].parameters.as_ref().unwrap();
        assert!(parameters
            .iter()
            .all(|parameter| parameter.documentation.is_none()));
    }

    #[test]
    fn test_signature_help_active_parameter() {
        crate::r_task(|| {
            harp::parse_eval_global("ark_sig <- function(a, b, c, d) NULL").unwrap();

            for (code, expected) in ACTIVE_PARAMETER_CASES {
                let help = signature_help_at_cursor(code).unwrap();

                assert_eq!((code, help.active_parameter), (code, Some(expected)));
                assert_eq!(
                    (code, help.signatures[0].active_parameter),
                    (code, Some(expected))
                );
            }

            harp::parse_eval_global("rm(ark_sig)").unwrap();
        })
    }

    #[test]
    fn test_static_signature_help_active_parameter() {
        for (code, expected) in ACTIVE_PARAMETER_CASES {
            let code = format!("ark_sig <- function(a, b, c, d) NULL\n{code}");
            let help = static_signature_help_at_cursor(&code)
                .unwrap()
                .help
                .unwrap();

            assert_eq!(label(&help), "ark_sig(a, b, c, d)");
            assert_eq!(
                (code.as_str(), help.active_parameter),
                (code.as_str(), Some(expected))
            );
            assert_eq!(
                (code.as_str(), help.signatures[0].active_parameter),
                (code.as_str(), Some(expected))
            );
        }
    }

    // With four formals, 5 is the out-of-range index used to prevent the
    // frontend from highlighting a parameter when none matches.
    const ACTIVE_PARAMETER_CASES: [(&str, u32); 6] = [
        ("ark_sig(@)", 0),
        ("ark_sig(1, @)", 1),
        ("ark_sig(c = 1, @)", 0),
        ("ark_sig(b = 1, 2, @)", 2),
        ("ark_sig(1, c = @)", 2),
        ("ark_sig(1, 2, 3, 4, @)", 5),
    ];

    #[test]
    fn test_session_signature_help_replaces_static_help() {
        crate::r_task(|| {
            let code = "ark_redef <- function(a) NULL\nark_redef(@)";

            // Failed session lookup leaves the static signature intact.
            let help = signature_help_at_cursor(code).unwrap();
            assert_eq!(label(&help), "ark_redef(a)");

            // Evaluated formals take precedence over the document's formals.
            harp::parse_eval_global("ark_redef <- function(x, y) NULL").unwrap();
            let help = signature_help_at_cursor(code).unwrap();
            assert_eq!(label(&help), "ark_redef(x, y)");

            // A session non-function says nothing about the callee, since R
            // skips non-function bindings when it looks up a function to call
            harp::parse_eval_global("ark_redef <- 1").unwrap();
            let help = signature_help_at_cursor(code).unwrap();
            assert_eq!(label(&help), "ark_redef(a)");

            harp::parse_eval_global("rm(ark_redef)").unwrap();
        })
    }

    #[test]
    fn test_session_signature_help_skips_non_function_bindings() {
        crate::r_task(|| {
            // R calls `base::match()` here, so the session describes it
            harp::parse_eval_global("match <- 1").unwrap();
            let StaticSignatureHelp { request, .. } =
                static_signature_help_at_cursor("match(@)").unwrap();
            let SignatureHelpReply::Fill(help) = r_signature_help(&request).unwrap() else {
                panic!("Expected session signature help");
            };
            assert_eq!(
                label(&help),
                "match(x, table, nomatch = NA, incomparables = NULL)"
            );

            harp::parse_eval_global("rm(match)").unwrap();
        })
    }

    #[test]
    fn test_argument_label_null() {
        crate::r_task(|| {
            let x = r_null();
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x = NULL"));
        })
    }

    #[test]
    fn test_argument_label_missing() {
        crate::r_task(|| {
            let x = harp::missing();
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x"));
        })
    }

    #[test]
    fn test_argument_label_symbol() {
        crate::r_task(|| {
            let x = r_symbol!("name");
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x = name"));

            let x = r_symbol!("hi there");
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x = `hi there`"));
        })
    }

    #[test]
    fn test_argument_label_call() {
        crate::r_task(|| {
            let x = RCall::new(r_symbol!("source"))
                .add(r_symbol!("exprs"))
                .param("local", r_symbol!("is_local"))
                .build();
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = source(exprs, local = is_local)"));
        })
    }

    #[test]
    fn test_argument_label_truncate() {
        crate::r_task(|| {
            let x = harp::missing();
            let name = "x".repeat(300);

            let mut expect = "x".repeat(200);
            expect.push_str("...");

            let label = argument_label(name, x);
            assert_eq!(label, expect);
        })
    }

    #[test]
    fn test_argument_label_vector() {
        crate::r_task(|| {
            let x = RObject::from(r_alloc_logical(3));
            r_lgl_poke(x.sexp, 0, 1);
            r_lgl_poke(x.sexp, 1, 0);
            r_lgl_poke(x.sexp, 2, r_lgl_na());
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = c(TRUE, FALSE, NA)"));

            let x = RObject::from(r_alloc_integer(3));
            r_int_poke(x.sexp, 0, 1);
            r_int_poke(x.sexp, 1, 2);
            r_int_poke(x.sexp, 2, r_int_na());
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = c(1L, 2L, NA)"));

            let x = RObject::from(r_alloc_double(3));
            r_dbl_poke(x.sexp, 0, 1.5);
            r_dbl_poke(x.sexp, 1, 2.5);
            r_dbl_poke(x.sexp, 2, r_dbl_na());
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = c(1.5, 2.5, NA)"));

            let x = RObject::from(r_alloc_complex(3));
            r_cpl_poke(x.sexp, 0, libr::Rcomplex { r: 1.0, i: 2.5 });
            r_cpl_poke(x.sexp, 1, libr::Rcomplex {
                r: r_dbl_positive_infinity(),
                i: r_dbl_negative_infinity(),
            });
            r_cpl_poke(x.sexp, 2, libr::Rcomplex {
                r: r_dbl_na(),
                i: 2.5,
            });
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = c(1+2.5i, Inf-Infi, NA+2.5i)"));

            let x = RObject::from(r_alloc_character(3));
            r_chr_poke(x.sexp, 0, unsafe { r_char!("hi") });
            r_chr_poke(x.sexp, 1, unsafe { r_char!("there") });
            r_chr_poke(x.sexp, 2, r_str_na());
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = c(\"hi\", \"there\", NA)"));
        })
    }

    #[test]
    fn test_argument_label_vector_truncate() {
        crate::r_task(|| {
            let x = RObject::from(r_alloc_integer(12));
            for i in 0..12 {
                r_int_poke(x.sexp, R_xlen_t::try_from(i).unwrap(), i);
            }

            let label = argument_label(String::from("x"), x.sexp);

            assert_eq!(
                label,
                String::from("x = c(0L, 1L, 2L, 3L, 4L, 5L, 6L, 7L, 8L, 9L, ...)")
            );
        })
    }

    #[test]
    fn test_argument_label_scalars() {
        crate::r_task(|| {
            let x = RObject::from(r_alloc_logical(1));
            r_lgl_poke(x.sexp, 0, 1);
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = TRUE"));

            let x = RObject::from(r_alloc_integer(1));
            r_int_poke(x.sexp, 0, 1);
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = 1L"));

            let x = RObject::from(r_alloc_double(1));
            r_dbl_poke(x.sexp, 0, 1.5);
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = 1.5"));

            let x = RObject::from(r_alloc_complex(1));
            r_cpl_poke(x.sexp, 0, libr::Rcomplex { r: 1.5, i: 2.5 });
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = 1.5+2.5i"));

            let x = RObject::from(r_alloc_character(1));
            r_chr_poke(x.sexp, 0, unsafe { r_char!("hi") });
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = \"hi\""));
        })
    }
}
