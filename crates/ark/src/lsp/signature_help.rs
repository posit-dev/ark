//
// signature.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use harp::call::r_expr_deparse;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::object::*;
use harp::pretty::*;
use harp::r_missing;
use harp::r_null;
use harp::utils::r_formals;
use harp::utils::r_is_function;
use harp::utils::r_is_object;
use harp::utils::r_type2char;
use harp::utils::r_typeof;
use harp::utils::sym_quote_invalid;
use harp::RSymbol;
use libr::*;
use log::info;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::ParameterInformation;
use tower_lsp::lsp_types::ParameterLabel;
use tower_lsp::lsp_types::SignatureHelp;
use tower_lsp::lsp_types::SignatureInformation;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::help::RHtmlHelp;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

// TODO: We should probably take a pass through `signature_help()` and rewrite it from
// the ground up using our more advanced rust / tree-sitter knowledge. It feels like it
// is the accumulation of a number of smaller changes that have resulted in something
// that is a bit hard to follow.

/// SAFETY: Requires access to the R runtime.
pub(crate) unsafe fn r_signature_help(
    context: &DocumentContext,
) -> anyhow::Result<Option<SignatureHelp>> {
    // Get document AST + completion position.
    let ast = &context.document.ast;

    // Find the node closest to the completion point.
    let node = ast.root_node();
    let Some(mut node) = node.find_closest_node_to_point(context.point) else {
        return Ok(None);
    };

    // If we landed on a comma before the cursor position, move to the next sibling node.
    // We need to check the position as, if the cursor is "on" the comma as in
    //
    //    foo (x = ,)
    //
    // then the current context is associated with 'x = ' and not with what follows
    // the comma.
    if node.node_type() == NodeType::Comma && node.start_position().is_before(context.point) {
        if let Some(sibling) = node.next_sibling() {
            node = sibling;
        }
    }

    if node.node_type() == NodeType::Anonymous(String::from(")")) {
        if let Some(sibling) = node.prev_sibling() {
            node = sibling;
        }
    }

    // Get the current node.
    let mut parent = match node.parent() {
        Some(parent) => parent,
        None => return Ok(None),
    };

    // Look for a call node. Keep track of other relevant context while we search for it.
    // We want to figure out which of the current formals is currently "active". This is
    // a bit tricky for R functions, as one can supply named and unnamed arguments in any
    // order. For example:
    //
    //   foo(a = 1, b, c = 2, d)
    //
    // is a legal function call, and so we cannot just count commas to see which
    // parameter is currently active.

    // The list of arguments that have been explicitly specified.
    let mut explicit_parameters = vec![];

    // The number of unnamed arguments that have been supplied.
    let mut num_unnamed_arguments = 0;

    // The active argument, if any. Relevant for cases where the cursor is lying after 'x = <...>',
    // so we know that 'x' must be active.
    let mut active_argument = None;

    // Whether we've found the child node we were looking for.
    let mut found_child = false;

    // The computed argument offset.
    let mut offset: Option<u32> = None;

    let call = loop {
        // If we found an 'arguments' node, then use that to infer the current offset.
        if parent.node_type() == NodeType::Arguments {
            // If the cursor lies upon a named argument, use that as an override.
            if let Some(name) = node.child_by_field_name("name") {
                let name = context.document.contents.node_slice(&name)?.to_string();
                active_argument = Some(name);
            }

            let mut cursor = parent.walk();
            let children = parent.children(&mut cursor);
            for child in children {
                if let Some(name) = child.child_by_field_name("name") {
                    // If this is a named argument, add it to the list.
                    let name = context.document.contents.node_slice(&name)?.to_string();
                    explicit_parameters.push(name);

                    // Subtract 1 from the number of unnamed arguments, as
                    // the next comma we see won't be associated with an
                    // unnamed argument.
                    num_unnamed_arguments -= 1;
                }

                // If we find a comma, add to the offset.
                if !found_child && child.node_type() == NodeType::Comma {
                    num_unnamed_arguments += 1;
                }

                // If we've now walked up to the current node, we can quit.
                if child == node {
                    found_child = true;
                }
            }
        }

        // If we find the 'call' node, we can quit.
        if parent.is_call() {
            break parent;
        }

        // Update.
        node = parent;
        parent = match node.parent() {
            Some(parent) => parent,
            None => return Ok(None),
        };
    };

    // Totally possible that `node.find_closest_node_to_point(context.point)` finds a
    // call node that is technically the closest node to the point, but is completely
    // before the point. We only want to provide signature help when inside `fn(<here>)`!
    if !is_within_call_parentheses(&context.point, &call) {
        return Ok(None);
    }

    // Get the left-hand side of the call.
    let callee = unwrap!(call.child(0), None => {
        return Ok(None);
    });

    // TODO: Should we search the document and / or the workspace index
    // before asking the R session for a definition? Which should take precedence?

    // Try to figure out what R object it's associated with.
    let code = context.document.contents.node_slice(&callee)?.to_string();

    let object = r_parse_eval(code.as_str(), RParseEvalOptions {
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

    // Get the formal parameter names associated with this function.
    let formals = r_formals(*object)?;

    // Get the help documentation associated with this function.
    let help = if callee.is_namespace_operator() {
        let package = callee.child_by_field_name("lhs").into_result()?;
        let package = context.document.contents.node_slice(&package)?.to_string();

        let topic = callee.child_by_field_name("rhs").into_result()?;
        let topic = context.document.contents.node_slice(&topic)?.to_string();

        RHtmlHelp::new(topic.as_str(), Some(package.as_str()))
    } else {
        let topic = context.document.contents.node_slice(&callee)?.to_string();
        RHtmlHelp::new(topic.as_str(), None)
    };

    // The signature label. We generate this as we walk through the
    // parameters, so we can more easily record offsets.
    let mut label = String::new();
    label.push_str(code.as_str());
    label.push_str("(");

    // Get the available parameters.
    let mut parameters = vec![];

    // Iterate over the documentation for each parameter, and add the relevant information.
    for (index, argument) in formals.iter().enumerate() {
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

        // If we had an explicit name, and this name matches the argument,
        // then update the offset now.
        if active_argument.as_ref() == Some(argument_name) {
            offset = Some(index as u32);
        }

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
    label.push_str(")");

    // Finally, if we don't have an offset, figure it out now.
    if offset.is_none() {
        for (index, argument) in formals.iter().enumerate() {
            // Was this argument explicitly provided? If so, skip it.
            if explicit_parameters.contains(&argument.name) {
                continue;
            }

            // Otherwise, check and see if we have any remaining commas.
            if num_unnamed_arguments > 0 {
                num_unnamed_arguments -= 1;
                continue;
            }

            // This is the argument.
            offset = Some(index as u32);
            break;
        }
    }

    // NOTE: It seems like the frontend still tries to highlight the first
    // parameter when the offset is set to none, so here we just force it to
    // match no available argument.
    if offset.is_none() {
        offset = Some((formals.len() + 1).try_into().unwrap_or_default());
    }

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

fn is_within_call_parentheses(x: &Point, node: &Node) -> bool {
    if node.node_type() != NodeType::Call {
        // This would be very weird
        log::error!("`is_within_call_parentheses()` called on a non-`call` node.");
        return false;
    }

    let Some(arguments) = node.child_by_field_name("arguments") else {
        return false;
    };

    let n_children = arguments.child_count();
    if n_children < 2 {
        log::error!("`arguments` node only has {n_children} children.");
        return false;
    }

    let open = arguments.child(1 - 1).unwrap();
    let close = arguments.child(n_children - 1).unwrap();

    if open.node_type() != NodeType::Anonymous(String::from("(")) {
        return false;
    }
    if close.node_type() != NodeType::Anonymous(String::from(")")) {
        return false;
    }

    x.is_after_or_equal(open.end_position()) && x.is_before_or_equal(close.start_position())
}

fn argument_label(name: String, value: SEXP) -> String {
    // Specially handle `R_MissingArg`, which looks like a `SYMSXP`,
    // but we don't want to add `=` to it. This is what we see when
    // there is no default argument (and also for `...`).
    if value == r_missing() {
        return name;
    }

    name + " = " + obj_label(value).as_str()
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
        _ => return type_label(x),
    }
}

fn type_label(x: SEXP) -> String {
    let out = r_type2char(r_typeof(x));
    let out = String::from("<") + out.as_str() + ">";
    out
}

fn s3_label(x: SEXP) -> String {
    match r_s3_pretty_class(x) {
        Ok(class) => class,
        Err(err) => {
            log::error!("{err:?}");
            String::from("<???>")
        },
    }
}

fn null_label() -> String {
    r_null_to_pretty_string()
}

fn list_label() -> String {
    // TODO: Should we try and do better? I doubt an inlined list is that common.
    String::from("<list>")
}

fn vec_label(x: SEXP, elt_to_pretty_string: fn(SEXP, isize) -> String) -> String {
    let size = r_length(x);
    let scalar = size == 1;

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

    if !scalar {
        out.push_str(")");
    }

    out
}

fn lgl_to_pretty_string(x: SEXP, i: isize) -> String {
    r_lgl_to_pretty_string(r_lgl_get(x, i))
}
fn int_to_pretty_string(x: SEXP, i: isize) -> String {
    r_int_to_pretty_string(r_int_get(x, i))
}
fn dbl_to_pretty_string(x: SEXP, i: isize) -> String {
    r_dbl_to_pretty_string(r_dbl_get(x, i))
}
fn cpl_to_pretty_string(x: SEXP, i: isize) -> String {
    r_cpl_to_pretty_string(r_cpl_get(x, i))
}
fn chr_to_pretty_string(x: SEXP, i: isize) -> String {
    r_str_to_pretty_string(r_chr_get(x, i))
}

fn sym_label(x: SEXP) -> String {
    if x == r_missing() {
        panic!("`R_MissingArg` should have been handled earlier.");
    }

    let x = RSymbol::new_unchecked(x);
    let x = String::from(x);
    let x = sym_quote_invalid(x.as_str());

    x
}

fn call_label(x: SEXP) -> String {
    match r_expr_deparse(x) {
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
    use harp::environment::R_ENVS;
    use harp::eval::r_parse_eval0;
    use harp::object::*;
    use harp::r_char;
    use harp::r_missing;
    use harp::r_null;
    use harp::r_symbol;
    use harp::test::r_test;
    use harp::RObject;
    use tower_lsp::lsp_types::ParameterLabel;

    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::lsp::signature_help::argument_label;
    use crate::lsp::signature_help::r_signature_help;
    use crate::test::point_from_cursor;

    #[test]
    fn test_basic_signature_help() {
        r_test(|| {
            let (text, point) = point_from_cursor("library(@)");
            let document = Document::new(&text, None);
            let context = DocumentContext::new(&document, point, None);

            let help = unsafe { r_signature_help(&context) };
            let help = help.unwrap().unwrap();
            assert_eq!(help.signatures.len(), 1);

            // Looking for the label offset into `library(package, ...etc)` for `package`
            let signature = help.signatures.get(0).unwrap();
            let label = &signature.parameters.as_ref().unwrap().get(0).unwrap().label;
            assert_eq!(label, &ParameterLabel::LabelOffsets([8, 15]));
        })
    }

    #[test]
    fn test_no_signature_help_outside_parentheses() {
        r_test(|| {
            let (text, point) = point_from_cursor("library@()");
            let document = Document::new(&text, None);
            let context = DocumentContext::new(&document, point, None);
            let help = unsafe { r_signature_help(&context) };
            let help = help.unwrap();
            assert!(help.is_none());

            let (text, point) = point_from_cursor("library()@");
            let document = Document::new(&text, None);
            let context = DocumentContext::new(&document, point, None);
            let help = unsafe { r_signature_help(&context) };
            let help = help.unwrap();
            assert!(help.is_none());
        })
    }

    #[test]
    fn test_signature_help_argument_defaults() {
        r_test(|| {
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
            r_parse_eval0(fun, R_ENVS.global).unwrap();

            let (text, point) = point_from_cursor("fn(@)");
            let document = Document::new(&text, None);
            let context = DocumentContext::new(&document, point, None);
            let help = unsafe { r_signature_help(&context) };
            let help = help.unwrap().unwrap();

            // Check expected signature label
            let signature = help.signatures.get(0).unwrap();
            assert_eq!(
                signature.label,
                String::from(
                    r#"fn(x, ..., by = NULL, value = 2.5, string = "foo", keep = c("yes", "no", "maybe"), length = c(2L, 3L), call = identity(x), lst = list(1, 2))"#
                )
            );

            // Clean up
            r_parse_eval0("rm(fn)", R_ENVS.global).unwrap();
        })
    }

    #[test]
    fn test_argument_label_null() {
        r_test(|| {
            let x = r_null();
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x = NULL"));
        })
    }

    #[test]
    fn test_argument_label_missing() {
        r_test(|| {
            let x = r_missing();
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x"));
        })
    }

    #[test]
    fn test_argument_label_symbol() {
        r_test(|| {
            let x = unsafe { r_symbol!("name") };
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x = name"));

            let x = unsafe { r_symbol!("hi there") };
            let label = argument_label(String::from("x"), x);
            assert_eq!(label, String::from("x = `hi there`"));
        })
    }

    #[test]
    fn test_argument_label_call() {
        r_test(|| {
            let x = unsafe {
                RCall::new(r_symbol!("source"))
                    .add(r_symbol!("exprs"))
                    .param("local", r_symbol!("is_local"))
                    .build()
            };
            let label = argument_label(String::from("x"), x.sexp);
            assert_eq!(label, String::from("x = source(exprs, local = is_local)"));
        })
    }

    #[test]
    fn test_argument_label_vector() {
        r_test(|| {
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
    fn test_argument_label_scalars() {
        r_test(|| {
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
