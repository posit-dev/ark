//
// srcref.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use core::f64;

use anyhow::anyhow;
use stdext::result::ResultExt;
use stdext::unwrap;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::vector::IntegerVector;
use crate::vector::Vector;
use crate::Environment;
use crate::RObject;

/// Structured representation of `srcref` integer vectors
/// 0-based offsets.
#[derive(Debug)]
pub struct SrcRef {
    pub inner: IntegerVector,

    /// Lines and virtual lines may differ if a `#line` directive is used in code:
    /// the former just counts actual lines, the latter respects the directive.
    /// `line` corresponds to `line_parsed` in the original base R srcref vector.
    pub line: std::ops::Range<u32>,
    pub line_virtual: std::ops::Range<u32>,

    /// Bytes and columns may be different due to multibyte characters.
    pub column: std::ops::Range<u32>,
    pub column_byte: std::ops::Range<u32>,
}

#[derive(Clone, Debug)]
pub struct SrcFile {
    pub inner: Environment,
}

impl SrcRef {
    pub fn srcfile(&self) -> anyhow::Result<SrcFile> {
        let Some(srcfile) = self.inner.object.get_attribute("srcfile") else {
            return Err(anyhow!("Can't find `srcfile` attribute"));
        };
        SrcFile::wrap(srcfile)
    }
}

// Takes user-facing object as input. The srcrefs are retrieved from
// attributes.
impl RObject {
    pub fn srcrefs(&self) -> anyhow::Result<Vec<SrcRef>> {
        let srcref = unwrap!(self.get_attribute("srcref"), None => {
            return Err(anyhow!("Can't find `srcref` attribute"));
        });

        crate::List::new(srcref.sexp)?
            .iter()
            .map(|x| SrcRef::try_from(RObject::view(x)))
            .collect()
    }
}

// Takes individual `srcref` attribute as input
impl TryFrom<RObject> for SrcRef {
    type Error = anyhow::Error;

    fn try_from(value: RObject) -> anyhow::Result<Self> {
        crate::r_assert_type(value.sexp, &[libr::INTSXP])?;
        crate::r_assert_capacity(value.sexp, 6)?;

        let value = IntegerVector::new(value)?;

        // The srcref values are adjusted to produce a `[ )` range as expected
        // by `std::ops::Range` that counts from 0. This is in contrast to the
        // ranges in `srcref` vectors which are 1-based `[ ]`.

        // Change from 1-based to 0-based counting
        let adjust_start = |i| (i - 1) as u32;

        // Change from 1-based to 0-based counting (-1) and make it an exclusive
        // boundary (+1). So essentially a no-op.
        let adjust_end = |i| i as u32;

        let line_start = adjust_start(value.get_value(0)?);
        let column_start = adjust_start(value.get_value(4)?);
        let column_byte_start = adjust_start(value.get_value(1)?);

        let line_end = adjust_end(value.get_value(2)?);
        let column_end = adjust_end(value.get_value(5)?);
        let column_byte_end = adjust_end(value.get_value(3)?);

        let line = std::ops::Range {
            start: line_start,
            end: line_end,
        };

        let line_parsed = if unsafe { value.len() >= 8 } {
            let line_parsed_start = adjust_start(value.get_value(6)?);
            let line_parsed_end = adjust_end(value.get_value(7)?);
            std::ops::Range {
                start: line_parsed_start,
                end: line_parsed_end,
            }
        } else {
            line.clone()
        };

        let column = std::ops::Range {
            start: column_start,
            end: column_end,
        };

        let column_byte = std::ops::Range {
            start: column_byte_start,
            end: column_byte_end,
        };

        Ok(Self {
            line: line_parsed,
            line_virtual: line,
            column,
            column_byte,
            inner: value,
        })
    }
}

/// Creates the same sort of srcfile object as with `parse(text = )`.
/// Takes code as an R string containing newlines, or as a R vector of lines.
impl SrcFile {
    pub fn wrap(value: RObject) -> anyhow::Result<SrcFile> {
        if value.kind() != libr::ENVSXP {
            return Err(anyhow!("Expected an environment, got {:?}", value.kind()));
        }
        if !value.inherits("srcfile") {
            return Err(anyhow!("Expected an srcfile, got {:?}", value.class()));
        }

        Ok(Self {
            inner: Environment::new(value),
        })
    }

    // Created by the R function `parse()`
    pub fn new_virtual(text: RObject) -> Self {
        let inner = RFunction::new("base", "srcfilecopy")
            .param("filename", "<text>")
            .param("lines", text)
            .call();

        // Unwrap safety: Should never fail, unless something is seriously wrong
        let inner = inner.unwrap();

        Self {
            inner: Environment::new(inner),
        }
    }

    // Created by the C-level parser
    pub fn new_virtual_empty_filename(text: RObject) -> Self {
        let inner = harp::Environment::new_empty();
        inner.bind("filename".into(), &RObject::from(""));
        inner.bind("lines".into(), &text);

        let inner: RObject = inner.into();

        harp::once! {
            static CLASS: RObject = crate::CharacterVector::create(vec!["srcfile", "srcfilecopy"]).into();
        }
        CLASS.with(|c| inner.set_attribute("class", c.sexp));

        Self {
            inner: Environment::new(inner),
        }
    }

    pub fn lines(&self) -> harp::Result<RObject> {
        RFunction::new("base", "getSrcLines")
            .add(self.inner.inner.sexp)
            .param("first", 1)
            .param("last", f64::INFINITY)
            .call()
    }

    pub fn filename(&self) -> anyhow::Result<String> {
        // In theory we should check if `filename` is relative, and prefix it
        // with `wd` in that case, if `wd` is set. For now we only use this
        // method to fetch our own URIs.
        self.inner.get("filename")?.try_into().anyhow()
    }
}

impl From<&str> for SrcFile {
    fn from(value: &str) -> Self {
        let input = crate::as_parse_text(value);
        SrcFile::new_virtual(input)
    }
}

impl From<&harp::CharacterVector> for SrcFile {
    fn from(value: &harp::CharacterVector) -> Self {
        SrcFile::new_virtual(value.object.clone())
    }
}

pub fn srcref_list_get(srcrefs: libr::SEXP, ind: isize) -> RObject {
    if crate::r_is_null(srcrefs) {
        return RObject::null();
    }

    if harp::r_length(srcrefs) <= ind {
        return RObject::null();
    }

    let result = harp::list_get(srcrefs, ind);

    if crate::r_is_null(result) {
        return RObject::null();
    }

    if unsafe { libr::TYPEOF(result) as u32 } != libr::INTSXP {
        return RObject::null();
    }

    if harp::r_length(result) < 6 {
        return RObject::null();
    }

    RObject::new(result)
}

// Some objects, such as calls to `{` and expression vectors returned by
// `parse()`, have a list of `srcref` objects attached as `srcref` attribute.
// This helper retrieves them if they exist.
pub fn get_srcref_list(call: libr::SEXP) -> Option<RObject> {
    let srcrefs = unsafe { libr::Rf_getAttrib(call, libr::R_SrcrefSymbol) };

    if unsafe { libr::TYPEOF(srcrefs) as u32 } == libr::VECSXP {
        return Some(RObject::new(srcrefs));
    }

    None
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use crate::srcref::SrcRef;

    #[test]
    fn test_srcref() {
        crate::r_task(|| {
            let exprs = crate::parse_exprs_with_srcrefs("foo\n\nś\nbar(\n\n)").unwrap();
            let srcrefs: Vec<SrcRef> = exprs.srcrefs().unwrap();
            let foo = &srcrefs[0];
            let utf8 = &srcrefs[1];
            let bar = &srcrefs[2];

            assert_eq!(foo.line, Range { start: 0, end: 1 });
            assert_eq!(foo.line_virtual, Range { start: 0, end: 1 });
            assert_eq!(foo.column, Range { start: 0, end: 3 });
            assert_eq!(foo.column_byte, Range { start: 0, end: 3 });

            // `column_byte` is different because the character takes up two bytes
            assert_eq!(utf8.line, Range { start: 2, end: 3 });
            assert_eq!(utf8.line_virtual, Range { start: 2, end: 3 });
            assert_eq!(utf8.column, Range { start: 0, end: 1 });
            assert_eq!(utf8.column_byte, Range { start: 0, end: 2 });

            // Ends on different lines
            assert_eq!(bar.line, Range { start: 3, end: 6 });
            assert_eq!(bar.line_virtual, Range { start: 3, end: 6 });
            assert_eq!(bar.column, Range { start: 0, end: 1 });
            assert_eq!(bar.column_byte, Range { start: 0, end: 1 });
        })
    }

    #[test]
    fn test_srcref_line_directive() {
        crate::r_task(|| {
            let exprs = crate::parse_exprs_with_srcrefs("foo\n#line 5\nbar").unwrap();
            let srcrefs: Vec<SrcRef> = exprs.srcrefs().unwrap();
            let foo = &srcrefs[0];
            let bar = &srcrefs[1];

            assert_eq!(foo.line, Range { start: 0, end: 1 });
            assert_eq!(foo.line_virtual, Range { start: 0, end: 1 });

            // Custom line via directive
            assert_eq!(bar.line, Range { start: 2, end: 3 });
            assert_eq!(bar.line_virtual, Range { start: 4, end: 5 });
        })
    }
}
