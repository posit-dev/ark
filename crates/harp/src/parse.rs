//
// parse.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;

use crate::line_ending::convert_line_endings;
use crate::line_ending::LineEnding;
use crate::protect::RProtect;
use crate::r_string;
use crate::srcref;
use crate::try_catch;
use crate::RObject;

pub struct RParseOptions {
    pub srcref: bool,
}

pub enum ParseResult {
    Complete(RObject),
    Incomplete,
}

impl Default for RParseOptions {
    fn default() -> Self {
        Self { srcref: false }
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
pub fn parse_exprs(code: &str) -> crate::Result<RObject> {
    parse_exprs_ext(code, Default::default())
}

/// Same but creates srcrefs
pub fn parse_exprs_with_srcrefs(code: &str) -> crate::Result<RObject> {
    parse_exprs_ext(code, RParseOptions { srcref: true })
}

fn parse_exprs_ext(code: &str, opts: RParseOptions) -> crate::Result<RObject> {
    let status = parse_status(code, opts)?;
    match status {
        ParseResult::Complete(x) => Ok(RObject::from(x)),
        ParseResult::Incomplete => Err(crate::Error::ParseError {
            code: code.to_string(),
            message: String::from("Incomplete code"),
        }),
    }
}

pub fn parse_status(code: &str, opts: RParseOptions) -> crate::Result<ParseResult> {
    unsafe {
        let mut status: libr::ParseStatus = libr::ParseStatus_PARSE_NULL;
        let mut protect = RProtect::new();
        let r_code = r_string!(convert_line_endings(code, LineEnding::Posix), &mut protect);

        let srcfile = if opts.srcref {
            srcref::new_srcfile_virtual(r_code)?
        } else {
            RObject::null()
        };

        let result: RObject =
            try_catch(|| libr::R_ParseVector(r_code, -1, &mut status, srcfile.sexp).into())?;

        match status {
            libr::ParseStatus_PARSE_OK => Ok(ParseResult::Complete(result)),
            libr::ParseStatus_PARSE_INCOMPLETE => Ok(ParseResult::Incomplete),
            libr::ParseStatus_PARSE_ERROR => Err(crate::Error::ParseSyntaxError {
                message: CStr::from_ptr(libr::get(libr::R_ParseErrorMsg).as_ptr())
                    .to_string_lossy()
                    .to_string(),
                line: libr::get(libr::R_ParseError) as i32,
            }),
            _ => {
                // Should not get here
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
    use crate::r_length;
    use crate::r_stringify;
    use crate::r_symbol;
    use crate::r_test;
    use crate::r_typeof;
    use crate::ParseResult;

    #[test]
    fn test_parse_status() {
        r_test! {
            assert_match!(
                parse_status("", Default::default()),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out.sexp), libr::EXPRSXP as u32);
                    assert_eq!(r_length(out.sexp), 0);
                }
            );

            // complete
            assert_match!(
                parse_status("force(42)", Default::default()),
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

            // incomplete
            assert_match!(
                parse_status("force(42", Default::default()),
                Ok(ParseResult::Incomplete)
            );

            // error
            assert_match!(
                parse_status("42 + _", Default::default()),
                Err(_) => {}
            );

            // "normal" syntax error
            assert_match!(
                parse_status("1+1\n*42", Default::default()),
                Err(crate::Error::ParseSyntaxError {message, line}) => {
                    assert!(message.contains("unexpected"));
                    assert_eq!(line, 2);
                }
            );

            // CRLF in the code string, like a file with CRLF line endings
            assert_match!(
                parse_status("x<-\r\n1\r\npi", Default::default()),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out.sexp), libr::EXPRSXP as u32);
                    assert_eq!(r_stringify(out.sexp, "").unwrap(), "expression(x <- 1, pi)");
                }
            );

            // CRLF inside a string literal in the code
            assert_match!(
                parse_status(r#"'a\r\nb'"#, Default::default()),
                Ok(ParseResult::Complete(out)) => {
                    assert_eq!(r_typeof(out.sexp), libr::EXPRSXP as u32);
                    assert_eq!(r_stringify(out.sexp, "").unwrap(), r#"expression("a\r\nb")"#);
                }
            );
        }
    }
}
