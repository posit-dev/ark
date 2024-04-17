//
// dap_variables.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::*;
use harp::r_symbol;
use harp::symbol::RSymbol;
use harp::utils::*;
use harp::vector::Vector;
use libr::*;
use stdext::unwrap;

use crate::thread::RThreadSafe;

pub struct RVariable {
    pub name: String,
    pub value: String,
    pub type_field: Option<String>,
    pub variables_reference_object: Option<RThreadSafe<RObject>>,
}

/// A "builder" pattern for `RVariable`
///
/// Useful because we generate `RVariable`s with various combinations of the possible
/// fields, and it's cleanest if we only have to specify the ones we actually have
/// values for.
struct RVariableBuilder {
    name: String,
    value: Option<String>,
    type_field: Option<String>,
    variables_reference_object: Option<RThreadSafe<RObject>>,
}

impl RVariableBuilder {
    fn new(name: String) -> Self {
        Self {
            name,
            value: None,
            type_field: None,
            variables_reference_object: None,
        }
    }

    fn value(mut self, x: String) -> Self {
        self.value = Some(x);
        self
    }

    fn type_field(mut self, x: String) -> Self {
        self.type_field = Some(x);
        self
    }

    fn variables_reference_object(mut self, x: RThreadSafe<RObject>) -> Self {
        self.variables_reference_object = Some(x);
        self
    }

    fn build(self) -> RVariable {
        let name = self.name;
        // `""` signals no value should be displayed
        let value = self.value.unwrap_or(String::from(""));
        let type_field = self.type_field;
        let variables_reference_object = self.variables_reference_object;

        RVariable {
            name,
            value,
            type_field,
            variables_reference_object,
        }
    }
}

/// Main entry point for `RVariable` collection by a `Variables` DAP request
///
/// Currently can collect variables for either:
/// - A frame environment, from a `FrameInfo`
/// - A recursive child of a frame environment, if that child is a bare list
///   or environment itself.
pub(super) fn object_variables(x: SEXP) -> Vec<RVariable> {
    match r_typeof(x) {
        ENVSXP => env_variables(x),
        VECSXP => list_variables(x),
        r_type => panic!(
            "Can't request variables for object of type '{}'.",
            r_type2char(r_type)
        ),
    }
}

fn env_variables(x: SEXP) -> Vec<RVariable> {
    let names = RObject::from(r_env_names(x));
    let names = Vec::<String>::try_from(names).unwrap_or(Vec::new());

    names
        .into_iter()
        .map(|name| env_binding_variable(name, x))
        .flatten()
        .collect()
}

fn env_binding_variable(name: String, x: SEXP) -> Option<RVariable> {
    if is_ignored_name(&name) {
        // Drop ignored names entirely
        return None;
    }

    let symbol = unsafe { r_symbol!(name) };

    match r_env_binding_is_active(x, symbol) {
        Ok(false) => {
            // Continue with standard environment variable creation
            ()
        },
        Ok(true) => {
            // We can't even extract out the object for active bindings so they
            // are handled extremely specially.
            return Some(active_binding_variable(name));
        },
        Err(err) => {
            log::error!("Can't determine if binding is active: {err:?}");
            return None;
        },
    }

    let x = r_envir_get(name.as_str(), x)?;
    let variable = object_variable(name, x);

    Some(variable)
}

fn list_variables(x: SEXP) -> Vec<RVariable> {
    let size = r_length(x) as usize;
    let names = indexed_names(x);

    let mut out = Vec::with_capacity(size);

    for (i, name) in names.into_iter().enumerate() {
        let elt = r_list_get(x, i as R_xlen_t);
        let variable = object_variable(name, elt);
        out.push(variable);
    }

    out
}

fn object_variable(name: String, x: SEXP) -> RVariable {
    if r_is_object(x) {
        object_variable_classed(name, x)
    } else {
        object_variable_bare(name, x)
    }
}

fn object_variable_classed(name: String, x: SEXP) -> RVariable {
    // TODO: Eventually add some support for classed values.
    // Right now we just display the class name.
    let class = object_class(x);

    let (value, type_field) = match class {
        Some(class) => (class.clone(), class.clone()),
        None => (String::from(""), String::from("<???>")),
    };

    RVariableBuilder::new(name)
        .value(value)
        .type_field(type_field)
        .build()
}

fn object_variable_bare(name: String, x: SEXP) -> RVariable {
    match r_typeof(x) {
        NILSXP => nil_variable(name, x),
        LGLSXP => vec_variable(name, x, LGLSXP),
        INTSXP => vec_variable(name, x, INTSXP),
        REALSXP => vec_variable(name, x, REALSXP),
        CPLXSXP => vec_variable(name, x, CPLXSXP),
        STRSXP => vec_variable(name, x, STRSXP),
        VECSXP => list_variable(name, x),
        SYMSXP => symbol_variable(name, x),
        LANGSXP => call_variable(name, x),
        PROMSXP => promise_variable(name, x),
        BCODESXP => bytecode_variable(name, x),
        EXPRSXP => expression_variable(name, x),
        LISTSXP => pairlist_variable(name, x),
        CLOSXP => closure_variable(name, x),
        ENVSXP => environment_variable(name, x),
        x_type => object_variable_bare_default(name, x_type),
    }
}

fn nil_variable(name: String, _x: SEXP) -> RVariable {
    RVariableBuilder::new(name)
        .value(String::from("NULL"))
        .type_field(String::from("<NULL>"))
        .build()
}

fn vec_variable(name: String, x: SEXP, x_type: SEXPTYPE) -> RVariable {
    RVariableBuilder::new(name)
        .value(vec_value(x, x_type))
        .type_field(vec_type_field(x_type))
        .build()
}

fn vec_type_field(x_type: SEXPTYPE) -> String {
    match x_type {
        LGLSXP => String::from("<logical>"),
        INTSXP => String::from("<integer>"),
        REALSXP => String::from("<double>"),
        CPLXSXP => String::from("<complex>"),
        STRSXP => String::from("<character>"),
        _ => std::unreachable!(),
    }
}

fn vec_value(x: SEXP, x_type: SEXPTYPE) -> String {
    let mut size = r_length(x);

    if size == 0 {
        return vec_value_empty(x_type);
    }

    // Cap the size
    let trim = size > 5;
    if trim {
        size = 5;
    }

    let mut out = "".to_string();

    match x_type {
        LGLSXP => lgl_fill_value(x, size, &mut out),
        INTSXP => int_fill_value(x, size, &mut out),
        REALSXP => dbl_fill_value(x, size, &mut out),
        CPLXSXP => cpl_fill_value(x, size, &mut out),
        STRSXP => chr_fill_value(x, size, &mut out),
        _ => std::unreachable!(),
    }

    if trim {
        out.push_str(", ...");
    }

    out
}

fn vec_value_empty(x_type: SEXPTYPE) -> String {
    match x_type {
        LGLSXP => String::from("logical(0)"),
        INTSXP => String::from("integer(0)"),
        REALSXP => String::from("double(0)"),
        CPLXSXP => String::from("complex(0)"),
        STRSXP => String::from("character(0)"),
        _ => std::unreachable!(),
    }
}

fn lgl_fill_value(x: SEXP, size: isize, out: &mut String) {
    for i in 0..size {
        let elt = r_lgl_get(x, i);
        let elt = lgl_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }
}

fn int_fill_value(x: SEXP, size: isize, out: &mut String) {
    for i in 0..size {
        let elt = r_int_get(x, i);
        let elt = int_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }
}

fn dbl_fill_value(x: SEXP, size: isize, out: &mut String) {
    for i in 0..size {
        let elt = r_dbl_get(x, i);
        let elt = dbl_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }
}

fn cpl_fill_value(x: SEXP, size: isize, out: &mut String) {
    for i in 0..size {
        let elt = r_cpl_get(x, i);
        let elt = cpl_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }
}

fn chr_fill_value(x: SEXP, size: isize, out: &mut String) {
    for i in 0..size {
        let elt = r_chr_get(x, i);
        let elt = str_to_string(elt);
        out.push_str(&elt);

        if i != size - 1 {
            out.push_str(", ");
        }
    }
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
        out.push_str("+");
    }

    let imaginary = dbl_to_string(x.i);
    out.push_str(&imaginary);
    out.push_str("i");

    out
}

fn str_to_string(x: SEXP) -> String {
    if x == r_str_na() {
        String::from("NA")
    } else {
        let mut out = String::from("\"");
        let elt = r_str_to_owned_utf8(x).unwrap_or(String::from("???"));
        out.push_str(&elt);
        out.push_str("\"");
        out
    }
}

fn list_variable(name: String, x: SEXP) -> RVariable {
    // This object can have children, and we know how to handle them
    let x = RThreadSafe::new(RObject::from(x));

    RVariableBuilder::new(name)
        .value(String::from("<list>"))
        .type_field(String::from("<list>"))
        .variables_reference_object(x)
        .build()
}

fn symbol_variable(name: String, x: SEXP) -> RVariable {
    let value = RSymbol::new_unchecked(x).to_string();
    let type_field = String::from("<symbol>");

    RVariableBuilder::new(name)
        .value(value)
        .type_field(type_field)
        .build()
}

fn call_variable(name: String, x: SEXP) -> RVariable {
    let value = unwrap!(call_value(x), Err(err) => {
        log::error!("Failed to format call value: {err:?}");
        String::from("<call>")
    });

    let type_field = String::from("<call>");

    RVariableBuilder::new(name)
        .value(value)
        .type_field(type_field)
        .build()
}

fn call_value(x: SEXP) -> anyhow::Result<String> {
    let x = RFunction::from(".ps.environment.describeCall")
        .add(x)
        .call()?;

    let x = String::try_from(x)?;

    Ok(x)
}

fn promise_variable(name: String, x: SEXP) -> RVariable {
    // Even if the promise hasn't been forced, the expression often contains
    // very useful information.
    // Practically it is typically:
    // - A symbol captured at the call site
    // - A call captured at the call site
    // - A simple object, like `NULL` or a scalar, that we know how to display
    let x = if r_promise_is_forced(x) {
        r_promise_value(x)
    } else {
        r_promise_expr(x)
    };

    if r_typeof(x) == PROMSXP {
        // Avoid any potential recursive weirdness
        return RVariableBuilder::new(name)
            .value(String::from("<promise>"))
            .type_field(String::from("<promise>"))
            .build();
    }

    // Let's assume we can display this object
    object_variable(name, x)
}

fn bytecode_variable(name: String, x: SEXP) -> RVariable {
    let x = r_bytecode_expr(x);

    if r_typeof(x) == BCODESXP {
        // Avoid any potential recursive weirdness
        return RVariableBuilder::new(name)
            .value(String::from("<bytecode>"))
            .type_field(String::from("<bytecode>"))
            .build();
    }

    // Let's assume we can display this object
    object_variable(name, x)
}

fn object_variable_bare_default(name: String, x_type: SEXPTYPE) -> RVariable {
    let class = r_type2char(x_type);

    let mut value = "<".to_string();
    value.push_str(&class);
    value.push_str(">");

    let type_field = value.clone();

    RVariableBuilder::new(name)
        .value(value)
        .type_field(type_field)
        .build()
}

fn closure_variable(name: String, _x: SEXP) -> RVariable {
    RVariableBuilder::new(name)
        .value(String::from("<function>"))
        .type_field(String::from("<function>"))
        .build()
}

fn environment_variable(name: String, x: SEXP) -> RVariable {
    // This object can have children, and we know how to handle them
    let x = RThreadSafe::new(RObject::from(x));

    RVariableBuilder::new(name)
        .value(String::from("<environment>"))
        .type_field(String::from("<environment>"))
        .variables_reference_object(x)
        .build()
}

fn pairlist_variable(name: String, _x: SEXP) -> RVariable {
    RVariableBuilder::new(name)
        .value(String::from("<pairlist>"))
        .type_field(String::from("<pairlist>"))
        .build()
}

fn expression_variable(name: String, _x: SEXP) -> RVariable {
    RVariableBuilder::new(name)
        .value(String::from("<expression>"))
        .type_field(String::from("<expression>"))
        .build()
}

fn active_binding_variable(name: String) -> RVariable {
    RVariableBuilder::new(name)
        .value(String::from("<active binding>"))
        .type_field(String::from("<active binding>"))
        .build()
}

fn object_class(x: SEXP) -> Option<String> {
    let Some(classes) = r_classes(x) else {
        // We've seen OBJECTs with no class attribute before
        return None;
    };

    let Ok(class) = classes.get(0) else {
        // Error means OOB error here (our weird Vector API, should probably be an Option?).
        log::error!("Detected length 0 class vector.");
        return None;
    };

    let Some(class) = class else {
        // `None` here means `NA` class value.
        log::error!("Detected `NA_character_` in a class vector.");
        return None;
    };

    let mut out = "<".to_string();
    out.push_str(&class);
    out.push_str(">");

    Some(out)
}

/// Return the names of a vector
///
/// If a name is empty, it is replaced with the 1-based index number instead
fn indexed_names(x: SEXP) -> Vec<String> {
    let names = RObject::from(r_names2(x));
    let size = r_length(names.sexp);

    let mut out = Vec::with_capacity(size as usize);

    for i in 0..size {
        let elt = r_chr_get(names.sexp, i);

        if elt == r_str_blank() {
            let elt = (i + 1).to_string();
            out.push(elt);
        } else {
            let elt = r_str_to_owned_utf8(elt).unwrap();
            out.push(elt);
        }
    }

    out
}

fn is_ignored_name(x: &str) -> bool {
    // Dots in signatures
    // TODO: It would be cool to show `<...>` for the dots, which could expand to show
    // the promises / values inside the dots without accidentally forcing anything.
    // See rlang's `capturedots()` for more about how to do this:
    // https://github.com/r-lib/rlang/blob/e5da30cb9fe54e020f0e122543466841c3ce6ea7/src/capture.c#L112
    if matches!(x, "...") {
        return true;
    }

    // S3 details passed through to generics and methods. See `?UseMethod`.
    // User can always print them in the console directly if they are advanced.
    // TODO: We could consider putting these in their own separate "Scope", which advanced
    // users might find useful.
    if matches!(
        x,
        ".Generic" | ".Method" | ".Class" | ".Group" | ".GenericCallEnv" | ".GenericDefEnv"
    ) {
        return true;
    }

    return false;
}

#[cfg(test)]
mod tests {
    use harp::environment::R_ENVS;
    use harp::eval::r_parse_eval0;
    use harp::exec::RFunction;
    use harp::exec::RFunctionExt;
    use harp::object::*;
    use harp::r_char;
    use harp::utils::r_envir_set;
    use libr::*;

    use crate::dap::dap_variables::cpl_to_string;
    use crate::dap::dap_variables::dbl_to_string;
    use crate::dap::dap_variables::env_binding_variable;
    use crate::dap::dap_variables::int_to_string;
    use crate::dap::dap_variables::lgl_to_string;
    use crate::dap::dap_variables::str_to_string;
    use crate::dap::dap_variables::vec_value;
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
    fn test_vec_value_methods() {
        r_test(|| unsafe {
            let x = RObject::from(r_alloc_integer(2));
            r_int_poke(x.sexp, 0, 1);
            r_int_poke(x.sexp, 1, r_int_na());
            assert_eq!(vec_value(x.sexp, INTSXP), String::from("1L, NA"));

            let x = RObject::from(r_alloc_double(5));
            r_dbl_poke(x.sexp, 0, 1.5);
            r_dbl_poke(x.sexp, 1, r_dbl_na());
            r_dbl_poke(x.sexp, 2, r_dbl_nan());
            r_dbl_poke(x.sexp, 3, r_dbl_positive_infinity());
            r_dbl_poke(x.sexp, 4, r_dbl_negative_infinity());
            assert_eq!(
                vec_value(x.sexp, REALSXP),
                String::from("1.5, NA, NaN, Inf, -Inf")
            );

            let x = RObject::from(r_alloc_character(2));
            r_chr_poke(x.sexp, 0, r_char!("hi"));
            r_chr_poke(x.sexp, 1, r_str_na());
            assert_eq!(vec_value(x.sexp, STRSXP), String::from("\"hi\", NA"))
        })
    }

    #[test]
    fn test_vec_value_truncation() {
        r_test(|| unsafe {
            let x = RObject::from(r_alloc_integer(6));
            r_int_poke(x.sexp, 0, 1);
            r_int_poke(x.sexp, 1, 2);
            r_int_poke(x.sexp, 2, 3);
            r_int_poke(x.sexp, 3, r_int_na());
            r_int_poke(x.sexp, 4, -1);
            r_int_poke(x.sexp, 5, 100);
            assert_eq!(
                vec_value(x.sexp, INTSXP),
                String::from("1L, 2L, 3L, NA, -1L, ...")
            )
        })
    }

    #[test]
    fn test_vec_value_empty() {
        r_test(|| unsafe {
            let x = RObject::from(r_alloc_logical(0));
            assert_eq!(vec_value(x.sexp, LGLSXP), String::from("logical(0)"));

            let x = RObject::from(r_alloc_integer(0));
            assert_eq!(vec_value(x.sexp, INTSXP), String::from("integer(0)"));

            let x = RObject::from(r_alloc_double(0));
            assert_eq!(vec_value(x.sexp, REALSXP), String::from("double(0)"));

            let x = RObject::from(r_alloc_complex(0));
            assert_eq!(vec_value(x.sexp, CPLXSXP), String::from("complex(0)"));

            let x = RObject::from(r_alloc_character(0));
            assert_eq!(vec_value(x.sexp, STRSXP), String::from("character(0)"));
        })
    }

    #[test]
    fn test_env_binding_variable_base() {
        r_test(|| unsafe {
            let env = RFunction::new("base", "new.env")
                .param("parent", R_ENVS.base)
                .call()
                .unwrap();

            let a = RObject::from(Rf_ScalarInteger(1));
            r_envir_set("a", a.sexp, env.sexp);
            let variable = env_binding_variable(String::from("a"), env.sexp).unwrap();
            assert_eq!(variable.name, String::from("a"));
            assert_eq!(variable.value, String::from("1L"));
            assert_eq!(variable.type_field, Some(String::from("<integer>")));

            let variable = env_binding_variable(String::from("b"), env.sexp);
            assert!(variable.is_none());
        })
    }

    #[test]
    fn test_env_binding_variable_classed() {
        r_test(|| unsafe {
            let env = RFunction::new("base", "new.env")
                .param("parent", R_ENVS.base)
                .call()
                .unwrap();

            let a = RObject::from(Rf_ScalarInteger(1));
            r_envir_set("a", a.sexp, env.sexp);

            let class = RObject::from(r_char!("foo"));
            let class = RObject::from(Rf_ScalarString(class.sexp));
            Rf_setAttrib(a.sexp, R_ClassSymbol, class.sexp);

            let variable = env_binding_variable(String::from("a"), env.sexp).unwrap();
            assert_eq!(variable.name, String::from("a"));
            assert_eq!(variable.value, String::from("<foo>"));
            assert_eq!(variable.type_field, Some(String::from("<foo>")));
        })
    }

    #[test]
    fn test_env_binding_variable_binding() {
        r_test(|| {
            let env = RFunction::new("base", "new.env")
                .param("parent", R_ENVS.base)
                .call()
                .unwrap();

            let function = r_parse_eval0("function() stop('oh no')", R_ENVS.base).unwrap();

            let _ = RFunction::new("base", "makeActiveBinding")
                .param("sym", "a")
                .param("fun", function)
                .param("env", env.sexp)
                .call()
                .unwrap();

            let variable = env_binding_variable(String::from("a"), env.sexp).unwrap();
            assert_eq!(variable.name, String::from("a"));
            assert_eq!(variable.value, String::from("<active binding>"));
            assert_eq!(variable.type_field, Some(String::from("<active binding>")));
        })
    }
}
