//
// json.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::cmp::min;

use libr::R_NamesSymbol;
use libr::Rf_allocVector;
use libr::Rf_setAttrib;
use libr::INTSXP;
use libr::LGLSXP;
use libr::NILSXP;
use libr::REALSXP;
use libr::SET_VECTOR_ELT;
use libr::STRSXP;
use libr::SYMSXP;
use libr::VECSXP;
use log::warn;
use serde_json::json;
use serde_json::Map;
use serde_json::Number;
use serde_json::Value;

use crate::exec::r_check_stack;
use crate::object::RObject;

/// Conversion to JSON values from an R object.
///
/// This is a recursive function that converts an R object to a JSON value. It
/// works with most primitive R data types, but isn't exhaustive and is designed
/// only to handle the conversion of data intendend for serialization over the
/// wire.
///
///  Most of the heavy lifting is done by RObject's conversion functions;
/// this function just handles the recursion and the conversion of lists.
///
/// Generally speaking:
///
/// - Zero-length vectors become JSON null values
///   - e.g.: c() -> null
/// - Length-one vectors become JSON scalars
///   - e.g.: 1L -> 1, TRUE -> true, "applesauce" -> "applesauce"
/// - Vectors of length > 1 become JSON arrays
///   - e.g.: c(1L, 2L, 3L) -> [1, 2, 3]
/// - Unnamed lists also become JSON arrays; note that, unlike atomic vectors,
///   these can contain elements of mixed types
///   - e.g.: list(1L, TRUE, "applesauce") -> [1, true, "applesauce"]
/// - Named lists become JSON maps/objects
///   - e.g.: list(a = 1L, b = TRUE, c = "applesauce") ->
///           {"a": 1, "b": true, "c": "applesauce"}
/// - Named lists with duplicate keys have the values combined into an array
///   - e.g.: list(a = 1L, a = 2L, a = 3L) -> {"a": [1, 2, 3]}
impl TryFrom<RObject> for Value {
    type Error = crate::error::Error;
    fn try_from(obj: RObject) -> Result<Self, Self::Error> {
        // Since this function is recursive, check the stack before we proceed
        // to make sure we aren't about to overflow it.
        r_check_stack(None)?;

        match obj.kind() {
            // Nil becomes JSON null
            NILSXP => Ok(Value::Null),

            // Integers (INTSXP) ---
            INTSXP => match obj.length() {
                // A length of 0 becomes JSON null
                0 => Ok(Value::Null),

                // A single integer becomes a JSON number
                1 => {
                    let value = unsafe { obj.to::<i32>()? };
                    Ok(Value::Number(value.into()))
                },

                // Multiple integers become integer vectors
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(match obj.get_i32(i)? {
                            Some(value) => value.into(),
                            None => Value::Null,
                        });
                    }
                    Ok(serde_json::Value::Array(arr))
                },
            },

            // Real / floating point numbers (REALSXP) ---
            REALSXP => match obj.length() {
                // A length of 0 becomes JSON null
                0 => Ok(Value::Null),

                // A single value becomes a JSON number
                1 => {
                    let value = unsafe { obj.to::<f64>()? };
                    // There's no try/into implicit conversion from f64 to a
                    // JSON number, but json! handles it.
                    Ok(json!(value))
                },

                // Multiple values become a vector
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(match obj.get_f64(i)? {
                            Some(value) => value.into(),
                            None => Value::Null,
                        });
                    }
                    Ok(serde_json::Value::Array(arr))
                },
            },

            // Logical / Boolean values (LGLSXP) ---
            LGLSXP => match obj.length() {
                // A length of 0 becomes JSON null
                0 => Ok(Value::Null),

                // A single value becomes a JSON true/false value
                1 => {
                    let value = unsafe { obj.to::<bool>()? };
                    Ok(Value::Bool(value))
                },

                // Multiple values become a vector
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(match obj.get_bool(i)? {
                            Some(value) => value.into(),
                            None => Value::Null,
                        });
                    }
                    Ok(serde_json::Value::Array(arr))
                },
            },

            // Symbols (SYMSXP) ---
            SYMSXP => {
                // Try to convert the symbol to a string; this uses PRINTNAME
                // under the hood
                let val = Option::<String>::try_from(obj)?;
                match val {
                    Some(value) => return Ok(Value::String(value)),
                    None => Ok(Value::Null),
                }
            },

            // Strings (STRSXP) ---
            STRSXP => match obj.length() {
                // A length of 0 becomes JSON null
                0 => Ok(Value::Null),

                // With exactly one value, convert to a string
                1 => {
                    let str = unsafe { obj.to::<String>()? };
                    Ok(Value::String(str))
                },

                // With multiple values, convert to a string array
                _ => {
                    let mut arr = Vec::<Value>::with_capacity(obj.length().try_into().unwrap());
                    let n = obj.length();
                    for i in 0..n {
                        arr.push(match obj.get_string(i)? {
                            Some(str) => Value::String(str),
                            None => Value::Null,
                        });
                    }
                    Ok(serde_json::Value::Array(arr))
                },
            },

            // Vectors/lists (VECSXP) ---
            VECSXP => match obj.length() {
                // A length of 0 becomes JSON null
                0 => Ok(Value::Null),

                _ => {
                    // See whether the object's values have names. We will try
                    // to convert named values into a JSON object (map); unnamed
                    // values become an array.
                    let mut names = obj.names();

                    // Check to see if all the names are empty. We want to treat
                    // this identically to an unnamed list.
                    let mut all_empty = true;
                    if let Some(names) = &names {
                        for name in names {
                            if let Some(name) = name {
                                if !name.is_empty() {
                                    all_empty = false;
                                    break;
                                }
                            }
                        }
                    }
                    if all_empty {
                        names = None;
                    }

                    match names {
                        Some(names) => {
                            // The object's values have names. Create a map.
                            let mut map = serde_json::Map::new();

                            // There's no guarantee that we have the same number
                            // of names as values, so be safe by taking the
                            // minimum of the two.
                            let n = min(obj.length(), names.len().try_into().unwrap());

                            // Create the map. Note that `Value::try_from` below
                            // will recurse into this function; this is how we
                            // handle arbitrarily deep lists.
                            //
                            // Consider: do we need to guard against
                            // self-referential lists?
                            for i in 0..n {
                                // Create the key-value pair to insert into the
                                // object; treat a missing name as an empty
                                // string.
                                let key = match &names[i as usize] {
                                    Some(name) => name.clone(),
                                    None => String::new(),
                                };
                                let val = Value::try_from(obj.vector_elt(i)?)?;

                                // Do we already have a value for this key? If
                                // so, we need to convert the existing value to
                                // an array and append the new value.
                                match map.get_mut(&key) {
                                    Some(existing) => match existing {
                                        Value::Array(arr) => {
                                            // The value is already an array; just
                                            // append the new value.
                                            arr.push(val);
                                        },
                                        _ => {
                                            // The value is not an array; create
                                            // one and append the new nad
                                            // existing values.
                                            let arr = vec![existing.clone(), val];
                                            map.insert(key, Value::Array(arr));
                                        },
                                    },
                                    None => {
                                        // We don't have a value for this key;
                                        // just insert the new value.
                                        map.insert(key, val);
                                    },
                                }
                            }
                            Ok(serde_json::Value::Object(map))
                        },
                        None => {
                            // The object's values don't have names. Create an array.
                            let n = obj.length();
                            let mut arr = Vec::<Value>::with_capacity(n.try_into().unwrap());

                            // Create the array. Note that `Value::try_from`
                            // below will recurse into this function to convert
                            // each element of the list to a value. Just like R
                            // list, JSON arrays can have elements of different
                            // types.
                            for i in 0..n {
                                arr.push(Value::try_from(obj.vector_elt(i)?)?)
                            }
                            Ok(serde_json::Value::Array(arr))
                        },
                    }
                },
            },

            // Everything else is not supported
            _ => {
                warn!(
                    "Attempt to serialize unsupported R SEXP (type {})",
                    obj.kind()
                );
                Ok(serde_json::Value::Null)
            },
        }
    }
}

/**
 * Convert a JSON number value to an R object.
 */
impl TryFrom<Number> for RObject {
    type Error = crate::error::Error;
    fn try_from(value: Number) -> Result<Self, Self::Error> {
        if value.is_i64() {
            // Prefer conversion to an R integer value if the number can be
            // represented as an integer.
            RObject::try_from(value.as_i64().unwrap())
        } else {
            // Otherwise, convert to an R real value.
            Ok(RObject::from(value.as_f64().unwrap()))
        }
    }
}

/**
 * Convert a vector of JSON values to an R object.
 */
impl TryFrom<Vec<Value>> for RObject {
    type Error = crate::error::Error;

    fn try_from(vals: Vec<Value>) -> Result<Self, Self::Error> {
        // Consider: currently, this creates an unnamed list. It would be
        // better, presuming that the values are all the same type, to create an
        // atomic vector of that type.
        unsafe {
            let list = RObject::from(Rf_allocVector(VECSXP, vals.len() as isize));
            for (i, val) in vals.iter().enumerate() {
                let val = RObject::try_from(val.clone())?;
                SET_VECTOR_ELT(list.sexp, i as isize, val.sexp);
            }
            return Ok(list);
        }
    }
}

impl TryFrom<Map<String, Value>> for RObject {
    type Error = crate::error::Error;

    fn try_from(map: Map<String, Value>) -> Result<Self, Self::Error> {
        // Convert the map's values into a vector of JSON values, then convert
        // that to an R object.
        let vals: Vec<Value> = map.values().cloned().collect();
        let list = RObject::try_from(vals)?;

        // Convert the map's keys into a vector of strings, then set the names
        // of the R object to that vector.
        let keys: Vec<String> = map.keys().cloned().collect();
        let names = RObject::from(keys);
        unsafe {
            Rf_setAttrib(list.sexp, R_NamesSymbol, names.sexp);
        }

        Ok(list)
    }
}

/**
 * Convert a JSON value to an R Object. Like the `TryFrom` implementation for
 * R to JSON, this is a recursive function that perfoms a lossy conversion.
 */
impl TryFrom<Value> for RObject {
    type Error = crate::error::Error;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::Null => Ok(RObject::from(())),
            Value::Bool(bool) => Ok(RObject::from(bool)),
            Value::Number(num) => RObject::try_from(num),
            Value::String(string) => Ok(RObject::from(string)),
            Value::Array(values) => RObject::try_from(values),
            Value::Object(map) => RObject::try_from(map),
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::exec::RFunction;
    use crate::exec::RFunctionExt;
    use crate::test::r_task;

    // Helper that takes an R expression (as a string), parses it, evaluates it,
    // and converts it to a JSON value. We use this extensively in the tests
    // below to ensure that the R objects are serialized to JSON correctly.
    fn r_to_json(expr: &str) -> Value {
        let evaluated = harp::parse_eval_global(expr).unwrap();

        // Convert the evaluated expression to a JSON value
        Value::try_from(evaluated).unwrap()
    }

    // Likewise, but for the reverse conversion.
    fn json_to_r(expr: &str) -> RObject {
        let json: Value = serde_json::from_str(expr).unwrap();

        RObject::try_from(json).unwrap()
    }

    // Deparses an R object to a string.
    fn deparse(obj: RObject) -> String {
        let result = RFunction::from("deparse").add(obj).call().unwrap();
        String::try_from(result).unwrap()
    }

    /// Core worker for JSON conversion tests. Takes an R expression and a JSON
    /// expression (both as strings) and ensures that the R expression converts
    /// to the JSON expression.
    ///
    /// - `r_expr`: The R expression to convert
    /// - `json_expr`: The JSON expression to convert to
    fn assert_r_matches_json(r_expr: &str, json_expr: &str) {
        let r = r_to_json(r_expr);
        let json: Value = serde_json::from_str(json_expr).unwrap();
        assert_eq!(r, json)
    }

    /// Core worker for R conversion tests.
    fn assert_json_matches_r(json_expr: &str, r_expr: &str) {
        let r = json_to_r(json_expr);
        assert_eq!(r_expr, deparse(r))
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_scalars() {
        // We expect length-one vectors to serialize to simple JSON scalars.
        r_task(|| {
            assert_r_matches_json("TRUE", "true");
            assert_r_matches_json("1L", "1");
            assert_r_matches_json("'applesauce'", "\"applesauce\"");
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_vectors() {
        // We expect vectors to serialize to JSON arrays.
        r_task(|| {
            assert_r_matches_json("c(1L, 2L, 3L)", "[1,2,3]");
            assert_r_matches_json("c('one', 'two')", "[\"one\", \"two\"]");
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_na_vectors() {
        // We expect vectors containing NA values to serialize to JSON arrays
        // with nulls.
        r_task(|| {
            assert_r_matches_json("c(1L, NA, 3L)", "[1, null, 3]");
            assert_r_matches_json("c('one', 'two', NA)", "[\"one\", \"two\", null]");
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_unnamed() {
        // We expect lists of unnamed elements to serialize to JSON arrays.
        r_task(|| {
            // List of integers
            assert_r_matches_json("list(1L, 2L, 3L)", "[1,2,3]");

            // List of logical values
            assert_r_matches_json("l <- list(TRUE, FALSE, TRUE); l", "[true, false, true]");

            // Empty names are ignored and treated as unnamed
            assert_r_matches_json(
                "l <- list('a', 'b', 'c'); names(l) <- c('', '', ''); l",
                "[\"a\", \"b\", \"c\"]",
            );

            // NA values in the names are ignored and treated as unnamed
            assert_r_matches_json(
                "l <- list('a', 'b', 'c'); names(l) <- c('', NA, ''); l",
                "[\"a\", \"b\", \"c\"]",
            );
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_mixed_types() {
        // We expect lists of mixed/heterogeneous types to serialize to JSON
        // arrays of mixed type.
        r_task(|| {
            assert_r_matches_json("list(1L, FALSE, 'cats')", "[1,false,\"cats\"]");
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_named() {
        // We expect named lists to serialize to JSON maps/objects.
        r_task(|| {
            assert_r_matches_json("list(a = 1L, b = 2L)", "{\"a\": 1, \"b\": 2}");
            assert_r_matches_json(
                "list(a = TRUE, b = 'cats')",
                "{\"a\": true, \"b\": \"cats\"}",
            );
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_duplicate() {
        // Duplicate keys are allowed in R lists, but not JSON objects. They
        // should be converted to JSON arrays.
        r_task(|| {
            assert_r_matches_json("list(a = 1L, a = 2L, a = 3L)", "{\"a\": [1, 2, 3]}");
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_json_lists_nested() {
        // When lists are nested, we expect them to serialize to nested JSON
        r_task(|| {
            assert_r_matches_json(
                "list(a = 1L, b = 2L, c = list(3L, 4L, 5L))",
                "{\"a\": 1, \"b\": 2, \"c\": [3,4,5]}",
            );
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_r_to_json_scalars() {
        r_task(|| {
            assert_json_matches_r("1", "1L");
            assert_json_matches_r("2.5", "2.5");
            assert_json_matches_r("true", "TRUE");
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_r_to_json_lists() {
        r_task(|| {
            assert_json_matches_r("[1,2,3]", "list(1L, 2L, 3L)");
            assert_json_matches_r(
                "[\"four\", \"five\", \"six\"]",
                "list(\"four\", \"five\", \"six\")",
            );
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_r_to_json_lists_mixed_types() {
        r_task(|| {
            assert_json_matches_r("[1,2,false]", "list(1L, 2L, FALSE)");
            assert_json_matches_r("[\"four\", \"five\", 6]", "list(\"four\", \"five\", 6L)");
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_r_to_json_objects() {
        r_task(|| {
            assert_json_matches_r(
                "{\"a\": 1, \"b\": 2, \"c\": 3}",
                "list(a = 1L, b = 2L, c = 3L)",
            );
            assert_json_matches_r(
                "{\"foo\": \"bar\", \"baz\": \"quux\", \"quuux\": false}",
                "list(foo = \"bar\", baz = \"quux\", quuux = FALSE)",
            );
        })
    }
}
