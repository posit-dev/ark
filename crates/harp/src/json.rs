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

            LGLSXP => match obj.length() {
                0 => Ok(Value::Null),
                1 => {
                    let value = unsafe { obj.to::<bool>()? };
                    Ok(Value::Bool(value))
                },
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(Value::Bool(obj.logical_elt(i)))
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

#[cfg(test)]
mod tests {

    use super::*;
    use crate::exec::RFunction;
    use crate::exec::RFunctionExt;
    use crate::r_test;

    fn r_to_json(expr: &str) -> Value {
        // Parse the string
        let parsed = unsafe {
            RFunction::new("base", "parse")
                .param("text", expr)
                .call()
                .unwrap()
        };
        // Evaluate it
        let evaluated = unsafe {
            RFunction::new("base", "eval")
                .param("expr", parsed)
                .call()
                .unwrap()
        };
        Value::try_from(evaluated).unwrap()
    }

    fn test_json_conversion(r_expr: &str, json_expr: &str) {
        let r = r_to_json(r_expr);
        let json: Value = serde_json::from_str(json_expr).unwrap();
        assert_eq!(r, json)
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_scalars() {
        r_test! {
            test_json_conversion("TRUE", "true");
            test_json_conversion("1L", "1");
            test_json_conversion("'applesauce'", "\"applesauce\"");
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_vectors() {
        r_test! {
            test_json_conversion(
                "c(1L, 2L, 3L)",
                "[1,2,3]"
            );
            test_json_conversion(
                "c('one', 'two')",
                "[\"one\", \"two\"]"
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_unnamed() {
        r_test! {
            test_json_conversion(
                "list(1L, 2L, 3L)",
                "[1,2,3]"
            );
            test_json_conversion(
                "list(TRUE, FALSE, TRUE)",
                "[true, false, true]"
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_named() {
        r_test! {
            test_json_conversion(
                "list(a = 1L, b = 2L)",
                "{\"a\": 1, \"b\": 2}"
            );
            test_json_conversion(
                "list(a = TRUE, b = 'cats')",
                "{\"a\": true, \"b\": \"cats\"}"
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_nested() {
        r_test! {
            test_json_conversion(
                "list(a = 1L, b = 2L, c = list(3L, 4L, 5L))",
                "{\"a\": 1, \"b\": 2, \"c\": [3,4,5]}"
            );
        }
    }
}
