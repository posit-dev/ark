//
// srcref.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use libr::SEXP;
use stdext::unwrap;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::vector::IntegerVector;
use crate::vector::Vector;
use crate::RObject;

/// Structured representation of `srcref` integer vectors
/// 0-based offsets.
#[derive(Debug)]
pub struct SrcRef {
    /// Lines and virtual lines may differ if a `#line` directive is used in code:
    /// the former just counts actual lines, the latter respects the directive.
    /// `line` corresponds to `line_parsed` in the original base R srcref vector.
    pub line: std::ops::Range<usize>,
    pub line_virtual: std::ops::Range<usize>,

    /// Bytes and columns may be different due to multibyte characters.
    pub column: std::ops::Range<usize>,
    pub column_byte: std::ops::Range<usize>,
}

// Takes user-facing object as input. The srcrefs are retrieved from
// attributes.
impl RObject {
    pub fn srcrefs(&self) -> anyhow::Result<Vec<SrcRef>> {
        let srcref = unwrap!(self.attr("srcref"), None => {
            return Err(anyhow!("Can't find `srcref` attribute"));
        });

        unsafe {
            crate::List::new(srcref.sexp)?
                .iter()
                .map(|x| SrcRef::try_from(RObject::view(x)))
                .collect()
        }
    }
}

// Takes individual `srcref` attribute as input
impl TryFrom<RObject> for SrcRef {
    type Error = anyhow::Error;

    fn try_from(value: RObject) -> anyhow::Result<Self> {
        crate::r_assert_type(value.sexp, &[libr::INTSXP])?;
        crate::r_assert_capacity(value.sexp, 6)?;

        let value = unsafe { IntegerVector::new(value)? };

        let line = std::ops::Range {
            start: (value.get_value(0)? - 1) as usize,
            end: (value.get_value(2)? - 1) as usize,
        };

        let line_parsed = if unsafe { value.len() >= 8 } {
            std::ops::Range {
                start: (value.get_value(6)? - 1) as usize,
                end: (value.get_value(7)? - 1) as usize,
            }
        } else {
            line.clone()
        };

        let column = std::ops::Range {
            start: (value.get_value(4)? - 1) as usize,
            end: value.get_value(5)? as usize,
        };

        let column_byte = std::ops::Range {
            start: (value.get_value(1)? - 1) as usize,
            end: value.get_value(3)? as usize,
        };

        Ok(Self {
            line: line_parsed,
            line_virtual: line,
            column,
            column_byte,
        })
    }
}

/// Creates the same sort of srcfile object as with `parse(text = )`.
/// Takes code as an R string containing newlines, or as a R vector of lines.
pub fn new_srcfile_virtual(code: SEXP) -> crate::Result<RObject> {
    RFunction::new("base", "srcfilecopy")
        .param("filename", "<text>")
        .param("lines", code)
        .call()
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use crate::srcref::SrcRef;
    use crate::test::r_test;

    #[test]
    fn test_srcref() {
        r_test(|| {
            let exprs = crate::parse_exprs_with_srcrefs("foo\n\nś\nbar(\n\n)").unwrap();
            let srcrefs: Vec<SrcRef> = exprs.srcrefs().unwrap();
            let foo = &srcrefs[0];
            let utf8 = &srcrefs[1];
            let bar = &srcrefs[2];

            assert_eq!(foo.line, Range { start: 0, end: 0 });
            assert_eq!(foo.line_virtual, Range { start: 0, end: 0 });
            assert_eq!(foo.column, Range { start: 0, end: 3 });
            assert_eq!(foo.column_byte, Range { start: 0, end: 3 });

            // `column_byte` is different because the character takes up two bytes
            assert_eq!(utf8.line, Range { start: 2, end: 2 });
            assert_eq!(utf8.line_virtual, Range { start: 2, end: 2 });
            assert_eq!(utf8.column, Range { start: 0, end: 1 });
            assert_eq!(utf8.column_byte, Range { start: 0, end: 2 });

            // Ends on different lines
            assert_eq!(bar.line, Range { start: 3, end: 5 });
            assert_eq!(bar.line_virtual, Range { start: 3, end: 5 });
            assert_eq!(bar.column, Range { start: 0, end: 1 });
            assert_eq!(bar.column_byte, Range { start: 0, end: 1 });
        })
    }

    #[test]
    fn test_srcref_line_directive() {
        r_test(|| {
            let exprs = crate::parse_exprs_with_srcrefs("foo\n#line 5\nbar").unwrap();
            let srcrefs: Vec<SrcRef> = exprs.srcrefs().unwrap();
            let foo = &srcrefs[0];
            let bar = &srcrefs[1];

            assert_eq!(foo.line, Range { start: 0, end: 0 });
            assert_eq!(foo.line_virtual, Range { start: 0, end: 0 });

            // Custom line via directive
            assert_eq!(bar.line, Range { start: 2, end: 2 });
            assert_eq!(bar.line_virtual, Range { start: 4, end: 4 });
        })
    }
}
