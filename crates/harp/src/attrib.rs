use libr::SEXP;

use crate::object::r_length;
use crate::r::attrib_poke;
use crate::r::attrib_poke_from;
use crate::r::fn_body;
use crate::r::fn_env;
use crate::r::fn_formals;
use crate::r::new_function;
use crate::r_null;
use crate::r_symbol;
use crate::RObject;

pub fn zap_srcref(x: SEXP) -> RObject {
    let x = RObject::new(x);

    match x.kind() {
        libr::CLOSXP => zap_srcref_fn(x.sexp),
        libr::LANGSXP => zap_srcref_call(x.sexp),
        libr::EXPRSXP => zap_srcref_expr(x.sexp),
        _ => x,
    }
}

fn zap_srcref_fn(x: SEXP) -> RObject {
    let formals = fn_formals(x);
    let body = fn_body(x);
    let env = fn_env(x);

    let new_body = zap_srcref(body);
    let out = RObject::new(new_function(formals, new_body.sexp, env));

    // Copy attributes from the original, but zap `srcref`
    attrib_poke_from(out.sexp, x);
    attrib_poke(out.sexp, r_symbol!("srcref"), r_null());

    out
}

fn zap_srcref_call(x: SEXP) -> RObject {
    unsafe {
        let x = RObject::view(x).shallow_duplicate();

        zap_srcref_attrib(x.sexp);

        if libr::CAR(x.sexp) == r_symbol!("function") {
            // Remove `call[[4]]` where the parser stores srcref information
            // for calls to `function`
            libr::SETCDR(libr::CDDR(x.sexp), r_null());
        }

        let mut node = x.sexp;
        while node != r_null() {
            libr::SETCAR(node, zap_srcref(libr::CAR(node)).sexp);
            node = libr::CDR(node);
        }

        x
    }
}

fn zap_srcref_expr(x: SEXP) -> RObject {
    let x = RObject::view(x).shallow_duplicate();

    zap_srcref_attrib(x.sexp);

    for i in 0..r_length(x.sexp) {
        let elt = harp::list_get(x.sexp, i);
        harp::list_poke(x.sexp, i, zap_srcref(elt).sexp);
    }

    x
}

fn zap_srcref_attrib(x: SEXP) {
    let x = RObject::view(x);
    x.set_attribute("srcfile", r_null());
    x.set_attribute("srcref", r_null());
    x.set_attribute("wholeSrcref", r_null());
}
