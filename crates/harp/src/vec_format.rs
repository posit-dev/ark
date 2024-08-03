//
// vec_format.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use libr::*;

use crate::object::*;
use crate::r_str_to_owned_utf8;
use crate::r_type2char;
use crate::r_typeof;

/// Opinionated atomic vector formatter used by the `Debug Variables` pane
pub fn vec_format(x: SEXP, limit: Option<R_xlen_t>) -> String {
    match r_typeof(x) {
        LGLSXP => lgl_format(x, limit),
        INTSXP => int_format(x, limit),
        REALSXP => dbl_format(x, limit),
        CPLXSXP => cpl_format(x, limit),
        STRSXP => chr_format(x, limit),
        x_type => std::panic!("Type '{}' is not supported.", r_type2char(x_type)),
    }
}

fn lgl_format(x: SEXP, limit: Option<R_xlen_t>) -> String {
    let (size, trimmed) = compute_format_size(x, limit);

    if size == 0 {
        return String::from("logical(0)");
    }

    let mut out = "".to_string();

    for i in 0..size {
        let elt = r_lgl_get(x, i);
        let elt = lgl_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }

    if trimmed {
        out.push_str(", ...");
    }

    out
}

fn int_format(x: SEXP, limit: Option<R_xlen_t>) -> String {
    let (size, trimmed) = compute_format_size(x, limit);

    if size == 0 {
        return String::from("integer(0)");
    }

    let mut out = "".to_string();

    for i in 0..size {
        let elt = r_int_get(x, i);
        let elt = int_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }

    if trimmed {
        out.push_str(", ...");
    }

    out
}

fn dbl_format(x: SEXP, limit: Option<R_xlen_t>) -> String {
    let (size, trimmed) = compute_format_size(x, limit);

    if size == 0 {
        return String::from("double(0)");
    }

    let mut out = "".to_string();

    for i in 0..size {
        let elt = r_dbl_get(x, i);
        let elt = dbl_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }

    if trimmed {
        out.push_str(", ...");
    }

    out
}

fn cpl_format(x: SEXP, limit: Option<R_xlen_t>) -> String {
    let (size, trimmed) = compute_format_size(x, limit);

    if size == 0 {
        return String::from("complex(0)");
    }

    let mut out = "".to_string();

    for i in 0..size {
        let elt = r_cpl_get(x, i);
        let elt = cpl_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }

    if trimmed {
        out.push_str(", ...");
    }

    out
}

fn chr_format(x: SEXP, limit: Option<R_xlen_t>) -> String {
    let (size, trimmed) = compute_format_size(x, limit);

    if size == 0 {
        return String::from("character(0)");
    }

    let mut out = "".to_string();

    for i in 0..size {
        let elt = r_chr_get(x, i);
        let elt = str_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }

    if trimmed {
        out.push_str(", ...");
    }

    out
}

fn compute_format_size(x: SEXP, limit: Option<R_xlen_t>) -> (R_xlen_t, bool) {
    let mut size = r_length(x);
    let mut trimmed = false;

    let Some(limit) = limit else {
        return (size, trimmed);
    };

    if size > limit {
        size = limit;
        trimmed = true;
    }

    (size, trimmed)
}

fn lgl_to_string(x: i32) -> String {
    if x == r_lgl_na() {
        String::from("NA")
    } else if x == 0 {
        String::from("FALSE")
    } else {
        String::from("TRUE")
    }
}

fn int_to_string(x: i32) -> String {
    if x == r_int_na() {
        String::from("NA")
    } else {
        x.to_string() + "L"
    }
}

fn dbl_to_string(x: f64) -> String {
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

fn cpl_to_string(x: Rcomplex) -> String {
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

fn str_to_string(x: SEXP) -> String {
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

#[cfg(test)]
mod tests {
    use harp::object::*;
    use harp::r_char;
    use libr::*;

    use crate::test::r_test;
    use crate::vec_format::cpl_to_string;
    use crate::vec_format::dbl_to_string;
    use crate::vec_format::int_to_string;
    use crate::vec_format::lgl_to_string;
    use crate::vec_format::str_to_string;
    use crate::vec_format::vec_format;

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

    #[test]
    fn test_vec_format_methods() {
        r_test(|| unsafe {
            let x = RObject::from(r_alloc_integer(2));
            r_int_poke(x.sexp, 0, 1);
            r_int_poke(x.sexp, 1, r_int_na());
            assert_eq!(vec_format(x.sexp, None), String::from("1L, NA"));

            let x = RObject::from(r_alloc_double(5));
            r_dbl_poke(x.sexp, 0, 1.5);
            r_dbl_poke(x.sexp, 1, r_dbl_na());
            r_dbl_poke(x.sexp, 2, r_dbl_nan());
            r_dbl_poke(x.sexp, 3, r_dbl_positive_infinity());
            r_dbl_poke(x.sexp, 4, r_dbl_negative_infinity());
            assert_eq!(
                vec_format(x.sexp, None),
                String::from("1.5, NA, NaN, Inf, -Inf")
            );

            let x = RObject::from(r_alloc_character(2));
            r_chr_poke(x.sexp, 0, r_char!("hi"));
            r_chr_poke(x.sexp, 1, r_str_na());
            assert_eq!(vec_format(x.sexp, None), String::from("\"hi\", NA"))
        })
    }

    #[test]
    fn test_vec_format_truncation() {
        r_test(|| {
            let x = RObject::from(r_alloc_integer(6));
            r_int_poke(x.sexp, 0, 1);
            r_int_poke(x.sexp, 1, 2);
            r_int_poke(x.sexp, 2, 3);
            r_int_poke(x.sexp, 3, r_int_na());
            r_int_poke(x.sexp, 4, -1);
            r_int_poke(x.sexp, 5, 100);
            assert_eq!(
                vec_format(x.sexp, Some(5)),
                String::from("1L, 2L, 3L, NA, -1L, ...")
            )
        })
    }

    #[test]
    fn test_vec_format_empty() {
        r_test(|| {
            let x = RObject::from(r_alloc_logical(0));
            assert_eq!(vec_format(x.sexp, None), String::from("logical(0)"));

            let x = RObject::from(r_alloc_integer(0));
            assert_eq!(vec_format(x.sexp, None), String::from("integer(0)"));

            let x = RObject::from(r_alloc_double(0));
            assert_eq!(vec_format(x.sexp, None), String::from("double(0)"));

            let x = RObject::from(r_alloc_complex(0));
            assert_eq!(vec_format(x.sexp, None), String::from("complex(0)"));

            let x = RObject::from(r_alloc_character(0));
            assert_eq!(vec_format(x.sexp, None), String::from("character(0)"));
        })
    }
}
