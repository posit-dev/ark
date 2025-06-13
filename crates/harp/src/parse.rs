//
// parse.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;

use itertools::Itertools;

use crate::line_ending::convert_line_endings;
use crate::line_ending::LineEnding;
use crate::parse_data::ParseData;
use crate::protect::RProtect;
use crate::r_string;
use crate::srcref;
use crate::try_catch;
use crate::vector::CharacterVector;
use crate::vector::Vector;
use crate::RObject;

pub struct RParseOptions {
    pub srcfile: Option<RObject>,
}

#[derive(Clone, Debug)]
pub enum ParseResult {
    Complete(RObject),
    Incomplete,
    SyntaxError { message: String, line: Option<i32> },
}

pub enum ParseInput<'a> {
    Text(&'a str),
    SrcFile(&'a srcref::SrcFile),
}

impl Default for RParseOptions {
    fn default() -> Self {
        Self { srcfile: None }
    }
}

/// Returns a single expression
pub fn parse_expr(code: &str) -> crate::Result<RObject> {
    unsafe {
        let exprs = parse_exprs(code)?;

        let n = libr::Rf_xlength(*exprs);
        if n != 1 {
            return Err(crate::Error::ParseError {
                code: code.to_string(),
                message: String::from("Expected a single expression, got {n}"),
            });
        }

        let expr = libr::VECTOR_ELT(*exprs, 0);
        Ok(expr.into())
    }
}

/// Returns an EXPRSXP vector
pub fn parse_exprs(text: &str) -> crate::Result<RObject> {
    parse_exprs_ext(&ParseInput::Text(text))
}

/// Same but creates srcrefs
pub fn parse_exprs_with_srcrefs(text: &str) -> crate::Result<RObject> {
    let srcfile = srcref::SrcFile::try_from(text)?;
    parse_exprs_ext(&ParseInput::SrcFile(&srcfile))
}

pub fn parse_exprs_ext<'a>(input: &ParseInput<'a>) -> crate::Result<RObject> {
    let status = parse_status(input)?;
    match status {
        ParseResult::Complete(x) => Ok(RObject::from(x)),
        ParseResult::Incomplete => Err(crate::Error::ParseError {
            code: parse_input_as_string(input).unwrap_or(String::from("Conversion error")),
            message: String::from("Incomplete code"),
        }),
        ParseResult::SyntaxError { message, line } => {
            Err(crate::Error::ParseSyntaxError { message, line })
        },
    }
}

pub fn parse_with_parse_data(text: &str) -> crate::Result<(ParseResult, ParseData)> {
    let srcfile = srcref::SrcFile::try_from(text)?;

    // Fill parse data in `srcfile` by side effect
    let status = parse_status(&ParseInput::SrcFile(&srcfile))?;

    let parse_data = ParseData::from_srcfile(&srcfile)?;

    Ok((status, parse_data))
}

pub fn parse_status<'a>(input: &ParseInput<'a>) -> crate::Result<ParseResult> {
    unsafe {
        // If we're parsing with srcrefs, keep parse data as well. This is the
        // default but it might have been overridden by the user.
        let _guard = harp::raii::RLocalOptionBoolean::new("keep.parse.data", true);

        let mut status: libr::ParseStatus = libr::ParseStatus_PARSE_NULL;

        let (text, srcfile) = match input {
            ParseInput::Text(text) => (as_parse_text(text), RObject::null()),
            ParseInput::SrcFile(srcfile) => (srcfile.lines()?, srcfile.inner.clone()),
        };

        let result: harp::Result<RObject> =
            try_catch(|| libr::R_ParseVector(text.sexp, -1, &mut status, srcfile.sexp).into());

        match result {
            Ok(result) => match status {
                libr::ParseStatus_PARSE_OK => Ok(ParseResult::Complete(result)),
                libr::ParseStatus_PARSE_INCOMPLETE => Ok(ParseResult::Incomplete),
                libr::ParseStatus_PARSE_ERROR => Ok(ParseResult::SyntaxError {
                    message: CStr::from_ptr(libr::get(libr::R_ParseErrorMsg).as_ptr())
                        .to_string_lossy()
                        .to_string(),
                    line: Some(libr::get(libr::R_ParseError) as i32),
                }),
                _ => {
                    // Should not get here
                    Err(crate::Error::ParseError {
                        code: parse_input_as_string(input)
                            .unwrap_or(String::from("String conversion error")),
                        message: String::from("Unknown parse error"),
                    })
                },
            },
            Err(error) => match error {
                // R's `raiseLexError()` or `raiseParseError()` throw classed R errors not
                // captured by the `PARSE_ERROR` variant of `ParseStatus` above. We promote
                // them to `ParseResult::SyntaxError` to simplify things for the caller.
                // https://github.com/r-devel/r-svn/blob/78a15b0be74c14161edfa1da65137b0a74a36df6/src/main/gram.c#L6748
                crate::Error::TryCatchError { message, class, .. }
                    if class
                        .as_ref()
                        .is_some_and(|class| class.iter().any(|class| class == "parseError")) =>
                {
                    Ok(ParseResult::SyntaxError {
                        message,
                        line: None,
                    })
                },
                error => Err(error),
            },
        }
    }
}

pub fn as_parse_text(text: &str) -> RObject {
    unsafe {
        let mut protect = RProtect::new();
        let input = r_string!(convert_line_endings(text, LineEnding::Posix), &mut protect);
        input.into()
    }
}

fn parse_input_as_string<'a>(input: &ParseInput<'a>) -> crate::Result<String> {
    Ok(match input {
        ParseInput::Text(text) => text.to_string(),
        ParseInput::SrcFile(srcfile) => {
            let lines = srcfile.lines()?;
            let lines = CharacterVector::new(lines)?;

            lines
                .iter()
                .map(|x| x.unwrap_or(String::from("NA")))
                .join("\n")
        },
    })
}

#[cfg(test)]
mod tests {
    use stdext::assert_match;

    use crate::parse::parse_input_as_string;
    use crate::parse::ParseInput;
    use crate::parse_status;
    use crate::r_length;
    use crate::r_stringify;
    use crate::r_symbol;
    use crate::r_typeof;
    use crate::srcref;
    use crate::ParseResult;

    #[test]
    fn test_parse_status() {
        crate::r_task(|| unsafe {
            assert_match!(
                parse_status(&ParseInput::Text("")),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out.sexp), libr::EXPRSXP as u32);
                    assert_eq!(r_length(out.sexp), 0);
                }
            );

            // Complete
            assert_match!(
                parse_status(&ParseInput::Text("force(42)")),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out.sexp), libr::EXPRSXP as u32);

                    let call = libr::VECTOR_ELT(out.sexp, 0);
                    assert_eq!(r_typeof(call), libr::LANGSXP as u32);
                    assert_eq!(libr::Rf_xlength(call), 2);
                    assert_eq!(libr::CAR(call), r_symbol!("force"));

                    let arg = libr::CADR(call);
                    assert_eq!(r_typeof(arg), libr::REALSXP as u32);
                    assert_eq!(*libr::REAL(arg), 42.0);
                }
            );

            // Incomplete
            assert_match!(
                parse_status(&ParseInput::Text("force(42")),
                Ok(ParseResult::Incomplete)
            );

            // Error
            // TODO: Can we turn this into a parse error? It seems to happen because
            // the parser calls `Rf_install()` to turn the name into a symbol, which
            // throws its own R error that doesn't get classed as a "parseError". It
            // currently shows a full backtrace because it looks like an unhandled error.
            // https://github.com/posit-dev/ark/issues/598
            assert_match!(parse_status(&ParseInput::Text("``")), Err(_));

            // "normal" syntax error
            assert_match!(
                parse_status(&ParseInput::Text("1+1\n*42")),
                Ok(ParseResult::SyntaxError {message, line}) => {
                    assert!(message.contains("unexpected"));
                    assert_eq!(line, Some(2));
                }
            );

            // `raiseParseError()` syntax error - pipe placeholder
            assert_match!(
                parse_status(&ParseInput::Text("42 + _")),
                Ok(ParseResult::SyntaxError {message, line}) => {
                    assert!(message.contains("pipe placeholder"));
                    assert_eq!(line, None);
                }
            );

            // `raiseParseError()` syntax error - unrecognized escape
            assert_match!(
                parse_status(&ParseInput::Text(r"'\s'")),
                Ok(ParseResult::SyntaxError {message, line}) => {
                    assert!(message.contains("unrecognized escape"));
                    assert_eq!(line, None);
                }
            );

            // CRLF in the code string, like a file with CRLF line endings
            assert_match!(
                parse_status(&ParseInput::Text("x<-\r\n1\r\npi")),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out.sexp), libr::EXPRSXP as u32);
                    assert_eq!(r_stringify(out.sexp, "").unwrap(), "expression(x <- 1, pi)");
                }
            );

            // CRLF inside a string literal in the code
            assert_match!(
                parse_status(&ParseInput::Text(r#"'a\r\nb'"#)),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out.sexp), libr::EXPRSXP as u32);
                    assert_eq!(r_stringify(out.sexp, "").unwrap(), r#"expression("a\r\nb")"#);
                }
            );
        })
    }

    #[test]
    fn test_parse_input_as_string() {
        crate::r_task(|| {
            assert_eq!(
                parse_input_as_string(&ParseInput::Text("foo\nbar")).unwrap(),
                "foo\nbar"
            );

            let input = srcref::SrcFile::try_from("foo\nbar").unwrap();
            assert_eq!(
                parse_input_as_string(&ParseInput::SrcFile(&input)).unwrap(),
                "foo\nbar"
            );
        });
    }
}
