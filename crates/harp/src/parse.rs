//
// parse.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;

use libr::SEXP;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::line_ending::convert_line_endings;
use crate::line_ending::LineEnding;
use crate::protect::RProtect;
use crate::r_string;
use crate::try_catch;
use crate::RObject;

pub enum ParseResult {
    Complete(SEXP),
    Incomplete,
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
pub fn parse_exprs(code: &str) -> crate::Result<RObject> {
    match parse_status(code)? {
        ParseResult::Complete(x) => {
            return Ok(RObject::from(x));
        },
        ParseResult::Incomplete => {
            return Err(crate::Error::ParseError {
                code: code.to_string(),
                message: String::from("Incomplete code"),
            });
        },
    };
}

/// This uses the R-level function `parse()` to create the srcrefs
pub fn parse_exprs_with_srcrefs(code: &str) -> crate::Result<RObject> {
    unsafe {
        let mut protect = RProtect::new();

        // Because `parse(text =)` doesn't allow `\r\n` even on Windows
        let code = convert_line_endings(code, LineEnding::Posix);
        let code = r_string!(code, protect);

        RFunction::new("base", "parse")
            .param("text", code)
            .param("keep.source", true)
            .call()
    }
}

pub fn parse_status(code: &str) -> crate::Result<ParseResult> {
    unsafe {
        let mut ps: libr::ParseStatus = libr::ParseStatus_PARSE_NULL;
        let mut protect = RProtect::new();
        let r_code = r_string!(convert_line_endings(code, LineEnding::Posix), &mut protect);

        let result: RObject =
            try_catch(|| libr::R_ParseVector(r_code, -1, &mut ps, libr::R_NilValue).into())?;

        match ps {
            libr::ParseStatus_PARSE_OK => Ok(ParseResult::Complete(result.sexp)),
            libr::ParseStatus_PARSE_INCOMPLETE => Ok(ParseResult::Incomplete),
            libr::ParseStatus_PARSE_ERROR => Err(crate::Error::ParseSyntaxError {
                message: CStr::from_ptr(libr::get(libr::R_ParseErrorMsg).as_ptr())
                    .to_string_lossy()
                    .to_string(),
                line: libr::get(libr::R_ParseError) as i32,
            }),
            _ => {
                // should not get here
                Err(crate::Error::ParseError {
                    code: code.to_string(),
                    message: String::from("Unknown parse error"),
                })
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::assert_match;
    use crate::parse_status;
    use crate::r_stringify;
    use crate::r_symbol;
    use crate::r_test;
    use crate::r_typeof;
    use crate::ParseResult;

    #[test]
    fn test_parse_status() {
        r_test! {
            // complete
            assert_match!(
                parse_status("force(42)"),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out), libr::EXPRSXP as u32);

                    let call = libr::VECTOR_ELT(out, 0);
                    assert_eq!(r_typeof(call), libr::LANGSXP as u32);
                    assert_eq!(libr::Rf_xlength(call), 2);
                    assert_eq!(libr::CAR(call), r_symbol!("force"));

                    let arg = libr::CADR(call);
                    assert_eq!(r_typeof(arg), libr::REALSXP as u32);
                    assert_eq!(*libr::REAL(arg), 42.0);
                }
            );

            // incomplete
            assert_match!(
                parse_status("force(42"),
                Ok(ParseResult::Incomplete)
            );

            // error
            assert_match!(
                parse_status("42 + _"),
                Err(_) => {}
            );

            // "normal" syntax error
            assert_match!(
                parse_status("1+1\n*42"),
                Err(crate::Error::ParseSyntaxError {message, line}) => {
                    assert!(message.contains("unexpected"));
                    assert_eq!(line, 2);
                }
            );

            // CRLF in the code string, like a file with CRLF line endings
            assert_match!(
                parse_status("x<-\r\n1\r\npi"),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out), libr::EXPRSXP as u32);
                    assert_eq!(r_stringify(out, "").unwrap(), "expression(x <- 1, pi)");
                }
            );

            // CRLF inside a string literal in the code
            assert_match!(
                parse_status(r#"'a\r\nb'"#),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out), libr::EXPRSXP as u32);
                    assert_eq!(r_stringify(out, "").unwrap(), r#"expression("a\r\nb")"#);
                }
            );
        }
    }
}