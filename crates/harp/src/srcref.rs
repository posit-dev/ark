//
// srcref.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use libr::SEXP;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::RObject;

/// Creates the same sort of srcfile object as with `parse(text = )`.
/// Takes code as an R string containing newlines, or as a R vector of lines.
pub fn new_srcfile_virtual(code: SEXP) -> crate::Result<RObject> {
    RFunction::new("base", "srcfilecopy")
        .param("filename", "<text>")
        .param("lines", code)
        .call()
}
