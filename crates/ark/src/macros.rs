//
// macros.rs
//
// Copyright (C) 2022 by RStudio, PBC
//
//

pub trait IntoResult<T, E> {
    fn into_result(self) -> Result<T, E>;
}

impl<T, E> IntoResult<T, E> for Result<T, E> {
    fn into_result(self) -> Result<T, E> { self }
}

impl<T> IntoResult<T, ()> for Option<T> {
    fn into_result(self) -> Result<T, ()> { self.ok_or(()) }
}

#[doc(hidden)]
pub fn _into_result<T, E>(object: impl IntoResult<T, E>) -> Result<T, E> {
    object.into_result()
}

macro_rules! unwrap {

    ($value: expr, $id: ident $error: block) => {
        match crate::macros::_into_result($value) {
            Ok(value) => value,
            Err($id) => $error,
        }
    };

    ($value: expr, $error: block) => {
        match crate::macros::_into_result($value) {
            Ok(value) => value,
            Err(_error) => $error,
        }
    }

}
pub(crate) use unwrap;

macro_rules! cstr {

    ($value:literal) => {{
        use std::os::raw::c_char;
        let value = concat!($value, "\0");
        value.as_ptr() as *mut c_char
    }};

    ($value:expr) => {{
        use std::os::raw::c_char;
        let value = [$value, "\0"].concat();
        value.as_ptr() as *mut c_char
    }};

}
pub(crate) use cstr;

macro_rules! cargs {

    ($($expr:expr),*) => {
        vec![$(cstr!($expr)),*]
    };

}
pub(crate) use cargs;
