/*
 * macros.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

#![allow(unused_macros, unused_imports)]

macro_rules! expect {

    ($value:expr, $fail:expr) => {
        match $value {
            Ok(value) => value,
            Err(error) => $fail,
        }
    }

}
pub(crate) use expect;

macro_rules! unwrap {

    ($value:expr, $fail:expr) => {
        match $value {
            Some(value) => value,
            None => $fail,
        }
    };

}
pub(crate) use unwrap;