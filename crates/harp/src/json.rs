//
// json.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use libR_sys::CHARSXP;
use libR_sys::INTSXP;
use libR_sys::NILSXP;
use libR_sys::STRSXP;
use serde_json::Value;

use crate::object::RObject;

impl TryFrom<RObject> for Value {
    type Error = crate::error::Error;
    fn try_from(obj: RObject) -> Result<Self, Self::Error> {
        // return nil
        match obj.kind() {
            NILSXP => return Ok(Value::Null),

            INTSXP => {
                let value = unsafe { obj.to::<i32>()? };
                return Ok(Value::Number(value.into()));
            },

            CHARSXP => match obj.length() {
                0 => return Ok(Value::Null),
                1 => {
                    let value = unsafe { obj.to::<String>()? };
                    return Ok(Value::String(value));
                },
                _ => {},
            },

            STRSXP => {
                let value = unsafe { obj.to::<String>()? };
                return Ok(Value::String(value));
            },
            _ => {},
        }
        Ok(Value::Null)
    }
}
