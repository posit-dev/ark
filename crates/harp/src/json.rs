//
// json.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::cmp::min;

use libR_sys::*;
use serde_json::json;
use serde_json::Value;

use crate::object::RObject;

impl TryFrom<RObject> for Value {
    type Error = crate::error::Error;
    fn try_from(obj: RObject) -> Result<Self, Self::Error> {
        match obj.kind() {
            NILSXP => Ok(Value::Null),

            INTSXP => match obj.length() {
                0 => Ok(Value::Null),
                1 => {
                    let value = unsafe { obj.to::<i32>()? };
                    Ok(Value::Number(value.into()))
                },
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(Value::Number(obj.integer_elt(i).into()));
                    }
                    Ok(serde_json::Value::Array(arr))
                },
            },

            REALSXP => match obj.length() {
                0 => Ok(Value::Null),
                1 => {
                    let value = unsafe { obj.to::<f64>()? };
                    Ok(json!(value))
                },
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(json!(obj.real_elt(i)))
                    }
                    Ok(serde_json::Value::Array(arr))
                },
            },

            CHARSXP => match obj.length() {
                0 => Ok(Value::Null),
                1 => {
                    let value = unsafe { obj.to::<String>()? };
                    Ok(Value::String(value))
                },
                _ => Ok(Value::Null),
            },

            SYMSXP => {
                let val = Option::<String>::try_from(obj)?;
                match val {
                    Some(value) => return Ok(Value::String(value)),
                    None => Ok(Value::Null),
                }
            },

            STRSXP => match obj.length() {
                0 => Ok(Value::Null),
                1 => {
                    let str = unsafe { obj.to::<String>()? };
                    Ok(Value::String(str))
                },
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(match obj.string_elt(i) {
                            Some(str) => Value::String(str),
                            None => Value::Null,
                        });
                    }
                    Ok(serde_json::Value::Array(arr))
                },
            },

            VECSXP => match obj.length() {
                0 => Ok(Value::Null),
                _ => {
                    let names = obj.names();
                    match names {
                        Some(names) => {
                            let mut map = serde_json::Map::new();
                            let n = min(obj.length(), names.len().try_into().unwrap());
                            for i in 0..n {
                                map.insert(
                                    names[i as usize].clone(),
                                    Value::try_from(obj.vector_elt(i))?,
                                );
                            }
                            Ok(serde_json::Value::Object(map))
                        },
                        None => {
                            let n = obj.length();
                            let mut arr = Vec::<Value>::with_capacity(n.try_into().unwrap());
                            for i in 0..n {
                                arr.push(Value::try_from(obj.vector_elt(i))?)
                            }
                            Ok(serde_json::Value::Array(arr))
                        },
                    }
                },
            },

            _ => Ok(serde_json::Value::Null),
        }
    }
}
