//
// format.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use libr::Rcomplex;
use libr::SEXP;

use crate::object::r_dbl_is_finite;
use crate::object::r_dbl_is_na;
use crate::object::r_dbl_is_nan;
use crate::object::r_int_na;
use crate::object::r_lgl_na;
use crate::object::r_str_na;
use crate::r_classes;
use crate::r_str_to_owned_utf8;
use crate::vector::Vector;

pub fn null_to_string() -> String {
    String::from("NULL")
}

pub fn lgl_to_string(x: i32) -> String {
    if x == r_lgl_na() {
        String::from("NA")
    } else if x == 0 {
        String::from("FALSE")
    } else {
        String::from("TRUE")
    }
}

pub fn int_to_string(x: i32) -> String {
    if x == r_int_na() {
        String::from("NA")
    } else {
        x.to_string() + "L"
    }
}

pub fn dbl_to_string(x: f64) -> String {
    if r_dbl_is_na(x) {
        String::from("NA")
    } else if r_dbl_is_nan(x) {
        String::from("NaN")
    } else if !r_dbl_is_finite(x) {
        if x.is_sign_positive() {
            String::from("Inf")
        } else {
            String::from("-Inf")
        }
    } else {
        x.to_string()
    }
}

pub fn cpl_to_string(x: Rcomplex) -> String {
    let mut out = String::from("");

    let real = dbl_to_string(x.r);
    out.push_str(&real);

    // If `x.i < 0`, use `-` from converting the dbl to string
    if r_dbl_is_na(x.i) || r_dbl_is_nan(x.i) || x.i >= 0.0 {
        out.push('+');
    }

    let imaginary = dbl_to_string(x.i);
    out.push_str(&imaginary);
    out.push('i');

    out
}

pub fn str_to_string(x: SEXP) -> String {
    if x == r_str_na() {
        String::from("NA")
    } else {
        let mut out = String::from("\"");
        let elt = r_str_to_owned_utf8(x).unwrap_or(String::from("???"));
        out.push_str(&elt);
        out.push('"');
        out
    }
}

pub fn s3_class_to_string(x: SEXP) -> harp::Result<String> {
    let Some(classes) = r_classes(x) else {
        // We've seen OBJECTs with no class attribute before
        return Err(harp::anyhow!("`x` is an OBJECT missing a class attribute."));
    };

    let Ok(class) = classes.get(0) else {
        // Error means OOB error here (our weird Vector API, should probably be an Option?).
        return Err(harp::anyhow!("Detected length 0 class vector."));
    };

    let Some(class) = class else {
        // `None` here means `NA` class value.
        return Err(harp::anyhow!("Detected `NA_character_` in a class vector."));
    };

    let mut out = "<".to_string();
    out.push_str(&class);
    out.push('>');

    Ok(out)
}

#[cfg(test)]
mod tests {
    use harp::object::*;
    use harp::r_char;
    use libr::*;

    use crate::format::cpl_to_string;
    use crate::format::dbl_to_string;
    use crate::format::int_to_string;
    use crate::format::lgl_to_string;
    use crate::format::str_to_string;
    use crate::test::r_test;

    #[test]
    fn test_to_string_methods() {
        r_test(|| unsafe {
            assert_eq!(lgl_to_string(1), String::from("TRUE"));
            assert_eq!(lgl_to_string(0), String::from("FALSE"));
            assert_eq!(lgl_to_string(r_lgl_na()), String::from("NA"));

            assert_eq!(int_to_string(1), String::from("1L"));
            assert_eq!(int_to_string(0), String::from("0L"));
            assert_eq!(int_to_string(-1), String::from("-1L"));
            assert_eq!(int_to_string(r_int_na()), String::from("NA"));

            assert_eq!(dbl_to_string(1.5), String::from("1.5"));
            assert_eq!(dbl_to_string(1.6), String::from("1.6"));
            assert_eq!(dbl_to_string(1.6888), String::from("1.6888"));
            assert_eq!(dbl_to_string(0.0), String::from("0"));
            assert_eq!(dbl_to_string(-1.5), String::from("-1.5"));
            assert_eq!(dbl_to_string(r_dbl_na()), String::from("NA"));
            assert_eq!(dbl_to_string(r_dbl_nan()), String::from("NaN"));
            assert_eq!(
                dbl_to_string(r_dbl_positive_infinity()),
                String::from("Inf")
            );
            assert_eq!(
                dbl_to_string(r_dbl_negative_infinity()),
                String::from("-Inf")
            );

            assert_eq!(
                cpl_to_string(Rcomplex { r: 1.5, i: 2.5 }),
                String::from("1.5+2.5i")
            );
            assert_eq!(
                cpl_to_string(Rcomplex { r: 0.0, i: 0.0 }),
                String::from("0+0i")
            );
            assert_eq!(
                cpl_to_string(Rcomplex { r: 1.0, i: -2.0 }),
                String::from("1-2i")
            );
            assert_eq!(
                cpl_to_string(Rcomplex {
                    r: r_dbl_na(),
                    i: r_dbl_nan()
                }),
                String::from("NA+NaNi")
            );
            assert_eq!(
                cpl_to_string(Rcomplex {
                    r: r_dbl_positive_infinity(),
                    i: r_dbl_negative_infinity()
                }),
                String::from("Inf-Infi")
            );

            let x = RObject::from(r_char!("abc"));
            assert_eq!(str_to_string(x.sexp), String::from("\"abc\""));
            let x = RObject::from(r_str_na());
            assert_eq!(str_to_string(x.sexp), String::from("NA"));
        })
    }
}
