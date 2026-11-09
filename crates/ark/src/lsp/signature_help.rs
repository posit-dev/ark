//
// signature.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use harp::call::RArgument;
use harp::eval::RParseEvalOptions;
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
use stdext::unwrap::IntoResult;
use tower_lsp_server::ls_types::Documentation;
use tower_lsp_server::ls_types::ParameterInformation;
use tower_lsp_server::ls_types::ParameterLabel;
use tower_lsp_server::ls_types::SignatureHelp;
use tower_lsp_server::ls_types::SignatureInformation;

use crate::lsp::call_arguments::active_parameter;
use crate::lsp::call_arguments::call_at;
use crate::lsp::call_arguments::CallAt;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::help::RHtmlHelp;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeTypeExt;

// TODO: We should probably take a pass through `signature_help()` and rewrite it from
// the ground up using our more advanced rust / tree-sitter knowledge. It feels like it
// is the accumulation of a number of smaller changes that have resulted in something
// that is a bit hard to follow.

/// SAFETY: Requires access to the R runtime.
pub(crate) fn r_signature_help(context: &DocumentContext) -> anyhow::Result<Option<SignatureHelp>> {
    let Some(CallAt { callee, arguments }) = call_at(context)? else {
        return Ok(None);
    };

    let code = &arguments.callee;

    let Some(formals) = r_callee_formals(code)? else {
        return Ok(None);
    };

    // Get the help documentation associated with this function.
    let help = if callee.is_namespace_operator() {
        let package = callee.child_by_field_name("lhs").into_result()?;
        let package = package.node_to_string(context.contents)?;

        let name = callee.child_by_field_name("rhs").into_result()?;
        let name = name.node_to_string(context.contents)?;

        RHtmlHelp::from_function(name.as_str(), Some(package.as_str()))
    } else {
        let name = callee.node_to_string(context.contents)?;
        RHtmlHelp::from_function(name.as_str(), None)
    };

    // The signature label. We generate this as we walk through the
    // parameters, so we can more easily record offsets.
    let mut label = String::new();
    label.push_str(code.as_str());
    label.push('(');

    // Get the available parameters.
    let mut parameters = vec![];

    // Iterate over the documentation for each parameter, and add the relevant information.
    for argument in formals.iter() {
        // Get argument components.
        let argument_name = &argument.name;
        let argument_value = &argument.value;
        let argument_label = argument_label(argument_name.clone(), argument_value.sexp);

        // Compute signature offsets.
        let start = label.len() as u32;
        let end = start + argument_label.len() as u32;

        // Add the argument label to the overall label.
        label.push_str(argument_label.as_str());
        label.push_str(", ");

        // Get documentation, if any.
        let mut documentation = None;
        if let Ok(Some(ref help)) = help {
            let markup = help.parameter(argument_name);
            if let Ok(Some(markup)) = markup {
                documentation = Some(Documentation::MarkupContent(markup));
            }
        }

        // Add the new parameter.
        parameters.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([start, end]),
            documentation,
        });
    }

    // Clean up the closing ', ', and add a closing parenthesis.
    if label.ends_with(", ") {
        label.pop();
        label.pop();
    }

    // Add a closing parenthesis.
    label.push(')');

    let formal_names: Vec<String> = formals
        .iter()
        .map(|argument| argument.name.clone())
        .collect();

    // NOTE: It seems like the frontend still tries to highlight the first
    // parameter when the offset is set to none, so here we just force it to
    // match no available argument.
    let offset = active_parameter(&arguments, &formal_names).unwrap_or(formals.len() + 1);
    let offset = Some(u32::try_from(offset).unwrap_or_default());

    let signature = SignatureInformation {
        label,
        documentation: None,
        parameters: Some(parameters),
        active_parameter: offset,
    };

    let help = SignatureHelp {
        signatures: vec![signature],
        active_signature: None,
        active_parameter: offset,
    };

    info!("{:?}", help);
    Ok(Some(help))
}

/// Formals of the function that `callee` evaluates to, or `None` if it can't
/// be evaluated safely or isn't a function.
///
/// Shared by signature help and custom completions so both match the cursor
/// against the same function's formals. Evaluates in the global environment,
/// not `console::selected_env()`.
///
/// SAFETY: Requires access to the R runtime.
pub(crate) fn r_callee_formals(callee: &str) -> anyhow::Result<Option<Vec<RArgument>>> {
    // TODO: Should we search the document and / or the workspace index
    // before asking the R session for a definition? Which should take precedence?

    let object = harp::parse_eval(callee, RParseEvalOptions {
        forbid_function_calls: true,
        ..Default::default()
    });

    let object = match object {
        Ok(object) => object,
        Err(err) => match err {
            // LHS of the call was too complex to evaluate.
            harp::error::Error::UnsafeEvaluationError(_) => return Ok(None),
            // LHS of the call evaluated to an error. Totally possible if the
            // user is writing pseudocode. Don't want to propagate an error here.
            _ => return Ok(None),
        },
    };

    if !r_is_function(*object) {
        // Not uncommon for tree-sitter to detect partially written code as a
        // call, like:
        // ---
        // mtcars$
        // plot(1:5)
        // ---
        // Where it detects `mtcars$plot` as the LHS of the call.
        // That is actually how R would parse this, but the user might be writing
        // `mtcars$` and requesting completions for the `$` when this occurs.
        // In these cases the `r_parse_eval()` above either errors or returns
        // something that isn't a function, so we ensure we have a function
        // before proceeding here.
        return Ok(None);
    }

    Ok(Some(r_formals(*object)?))
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

    use crate::fixtures::point_from_cursor;
    use crate::lsp::document_context::TestDocument;
    use crate::lsp::signature_help::argument_label;
    use crate::lsp::signature_help::r_signature_help;

    #[test]
    fn test_basic_signature_help() {
        crate::r_task(|| {
            let (text, point) = point_from_cursor("library(@)");
            let doc = TestDocument::new(&text);
            let context = doc.context(point);

            let help = r_signature_help(&context);
            let help = help.unwrap().unwrap();
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
        crate::r_task(|| {
            let (text, point) = point_from_cursor("library@()");
            let doc = TestDocument::new(&text);
            let context = doc.context(point);
            let help = r_signature_help(&context);
            let help = help.unwrap();
            assert!(help.is_none());

            let (text, point) = point_from_cursor("library()@");
            let doc = TestDocument::new(&text);
            let context = doc.context(point);
            let help = r_signature_help(&context);
            let help = help.unwrap();
            assert!(help.is_none());
        })
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

            let (text, point) = point_from_cursor("fn(@)");
            let doc = TestDocument::new(&text);
            let context = doc.context(point);
            let help = r_signature_help(&context);
            let help = help.unwrap().unwrap();

            // Check expected signature label
            let signature = help.signatures.first().unwrap();
            assert_eq!(
                signature.label,
                String::from(
                    r#"fn(x, ..., by = NULL, value = 2.5, string = "foo", keep = c("yes", "no", "maybe"), length = c(2L, 3L), call = identity(x), lst = list(1, 2))"#
                )
            );

            // Clean up
            harp::parse_eval_global("rm(fn)").unwrap();
        })
    }

    #[test]
    fn test_signature_help_active_parameter() {
        crate::r_task(|| {
            harp::parse_eval_global("ark_sig <- function(a, b, c, d) NULL").unwrap();

            // 5 is the no-match sentinel, `formals.len() + 1`
            let cases = [
                ("ark_sig(@)", 0),
                ("ark_sig(1, @)", 1),
                ("ark_sig(c = 1, @)", 0),
                ("ark_sig(b = 1, 2, @)", 2),
                ("ark_sig(1, c = @)", 2),
                ("ark_sig(1, 2, 3, 4, @)", 5),
            ];

            for (code, expected) in cases {
                let (text, point) = point_from_cursor(code);
                let doc = TestDocument::new(&text);
                let context = doc.context(point);
                let help = r_signature_help(&context).unwrap().unwrap();

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
