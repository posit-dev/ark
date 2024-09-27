//
// size.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashSet;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::u32;

use libc::c_double;
use libr::*;

use crate::environment::BindingValue;
use crate::environment::Environment;
use crate::environment::R_ENVS;
use crate::list_get;
use crate::object::r_chr_get;
use crate::object::r_length;
use crate::r_is_altrep;
use crate::r_symbol;
use crate::r_typeof;
use crate::RObject;

// A re-implementation of lobstr obj_size
// https://github.com/r-lib/lobstr/blob/9ee1481c9d322fe0a5c798f3f20e608622ddc257/src/size.cpp#L201
//
// `utils::object.size()` is too slow on large datasets and this code path is used trough the
// variables pane which required more performance.
// See for more info.
pub fn r_size(x: SEXP) -> harp::Result<usize> {
    let mut seen: HashSet<SEXP> = HashSet::new();

    let sizeof_node: f64 = harp::parse_eval_base("as.vector(utils::object.size(quote(expr = )))")
        .and_then(|x| x.try_into())?;

    let sizeof_vector: f64 = harp::parse_eval_base("as.vector(utils::object.size(logical()))")
        .and_then(|x| x.try_into())?;

    // The tree-walking implementation potentially violates R internals,
    // so we protect against errors thrown by R (and hope for no crash).
    // https://github.com/posit-dev/positron/issues/4686
    harp::try_catch(|| {
        obj_size_tree(
            x,
            R_ENVS.global,
            sizeof_node as usize,
            sizeof_vector as usize,
            &mut seen,
            0,
        )
    })
}

fn obj_size_tree(
    x: SEXP,
    base_env: SEXP,
    sizeof_node: usize,
    sizeof_vector: usize,
    seen: &mut HashSet<SEXP>,
    depth: u32,
) -> usize {
    // In case there's a nullptr, return 0
    if x.is_null() {
        return 0;
    }

    // NILSXP is a singleton, so occupies no space. Similarly SPECIAL and
    // BUILTIN are fixed and unchanging
    match r_typeof(x) {
        NILSXP | SPECIALSXP | BUILTINSXP => return 0,
        _ => {},
    };

    // Don't count objects that we've seen before
    if !seen.insert(x) {
        return 0;
    };

    // Use sizeof(SEXPREC) and sizeof(VECTOR_SEXPREC) computed in R.
    // CHARSXP are treated as vectors for this purpose
    let mut size = if unsafe { Rf_isVector(x) == Rboolean_TRUE } || r_typeof(x) == CHARSXP {
        sizeof_vector
    } else {
        sizeof_node
    };

    if r_is_altrep(x) {
        let klass = unsafe { libr::ALTREP_CLASS(x) };
        size += 3 * size_of::<SEXP>();

        size += obj_size_tree(klass, base_env, sizeof_node, sizeof_vector, seen, depth + 1);

        size += obj_size_tree(
            unsafe { libr::R_altrep_data1(x) },
            base_env,
            sizeof_node,
            sizeof_vector,
            seen,
            depth + 1,
        );
        size += obj_size_tree(
            unsafe { libr::R_altrep_data2(x) },
            base_env,
            sizeof_node,
            sizeof_vector,
            seen,
            depth + 1,
        );

        return size;
    }

    if r_typeof(x) != CHARSXP {
        size += obj_size_tree(
            unsafe { libr::ATTRIB(x) },
            base_env,
            sizeof_node,
            sizeof_vector,
            seen,
            depth + 1,
        );
    }

    match r_typeof(x) {
        LGLSXP | INTSXP => {
            size += v_size(r_length(x) as usize, size_of::<c_int>());
        },
        REALSXP => {
            size += v_size(r_length(x) as usize, size_of::<c_double>());
        },
        CPLXSXP => {
            size += v_size(r_length(x) as usize, size_of::<Rcomplex>());
        },
        RAWSXP => {
            size += v_size(r_length(x) as usize, 1);
        },
        // Strings
        STRSXP => {
            size += v_size(r_length(x) as usize, size_of::<SEXP>());
            for i in 0..r_length(x) {
                size += obj_size_tree(
                    r_chr_get(x, i),
                    base_env,
                    sizeof_node,
                    sizeof_vector,
                    seen,
                    depth + 1,
                );
            }
        },
        CHARSXP => {
            size += v_size(r_length(x) as usize + 1, 1);
        },
        // Generic vectors
        VECSXP | EXPRSXP | WEAKREFSXP => {
            size += v_size(r_length(x) as usize, size_of::<SEXP>());
            for i in 0..r_length(x) {
                size += obj_size_tree(
                    list_get(x, i),
                    base_env,
                    sizeof_node,
                    sizeof_vector,
                    seen,
                    depth + 1,
                )
            }
        },
        // Nodes
        // https://github.com/wch/r-source/blob/master/src/include/Rinternals.h#L237-L249
        // All have enough space for three SEXP pointers
        DOTSXP | LISTSXP | LANGSXP => {
            // Needed for DOTSXP
            if unsafe { x != libr::R_MissingArg } {
                let mut cons = x;
                while is_linked_list(cons) {
                    if cons != x {
                        size += sizeof_node
                    }
                    size += obj_size_tree(
                        unsafe { libr::TAG(cons) },
                        base_env,
                        sizeof_node,
                        sizeof_vector,
                        seen,
                        depth + 1,
                    );
                    size += obj_size_tree(
                        unsafe { libr::CAR(cons) },
                        base_env,
                        sizeof_node,
                        sizeof_vector,
                        seen,
                        depth + 1,
                    );
                    cons = unsafe { libr::CDR(cons) };
                }
                // Handle non-nil CDRs
                size += obj_size_tree(cons, base_env, sizeof_node, sizeof_vector, seen, depth + 1);
            }
        },
        BCODESXP => {
            size += obj_size_tree(
                unsafe { libr::TAG(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
            size += obj_size_tree(
                unsafe { libr::CAR(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
            size += obj_size_tree(
                unsafe { libr::CDR(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
        },
        // Environments
        ENVSXP => {
            if x == R_ENVS.base ||
                x == R_ENVS.global ||
                x == R_ENVS.empty ||
                x == base_env ||
                is_namespace(x)
            {
                return 0;
            }

            // We can't access environment bindings values using `CAR` because this requires knowledge
            // about internals of environments.
            // This also means we won't count the size of internal implementation details of environments,
            // such as the pre-allocated size of hash tables, etc.

            for binding in Environment::new(RObject::view(x))
                .iter()
                .filter_map(|x| x.ok())
            {
                // `binding.name`s are SYMSXP, for which we don't need to add to size.
                // We do add a node size for each binding though, because that's
                // the size the internal pairlist uses for each element.
                size += sizeof_node;

                size += match binding.value {
                    // For active bindings, we compute the size of the function
                    BindingValue::Active { fun } => {
                        obj_size_tree(fun.sexp, base_env, sizeof_node, sizeof_vector, seen, depth)
                    },
                    // `obj_size_tree` is aware of altrep objects.
                    BindingValue::Altrep { object, .. } => obj_size_tree(
                        object.sexp,
                        base_env,
                        sizeof_node,
                        sizeof_vector,
                        seen,
                        depth + 1,
                    ),
                    // `object_size_tree` is aware of promise objects.
                    // The environment iterator will automatically return `PRVALUE` as
                    // a `Standard` binding though. So this is only seeing unevaluated promises.
                    // For evaluated promises, we are not counting the size of `PRCODE`, but hopefully
                    // their sizes are negligible, mostly just symbols or small expressions.
                    BindingValue::Promise { promise } => obj_size_tree(
                        promise.sexp,
                        base_env,
                        sizeof_node,
                        sizeof_vector,
                        seen,
                        depth + 1,
                    ),
                    // Immediate bindings are expanded, thus we might overestimate the size of
                    // environments that use this kind of bindings.
                    // See more in https://github.com/r-devel/r-svn/blob/31340c871c7df54e45bfc7c4f49d09bb5806ec70/doc/notes/immbnd.md
                    BindingValue::Standard { object, .. } => obj_size_tree(
                        object.sexp,
                        base_env,
                        sizeof_node,
                        sizeof_vector,
                        seen,
                        depth,
                    ),
                }
            }

            size += obj_size_tree(
                unsafe { libr::ENCLOS(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
        },
        // Functions
        CLOSXP => {
            size += obj_size_tree(
                unsafe { libr::FORMALS(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
            // BODY is either an expression or byte code
            size += obj_size_tree(
                unsafe { libr::BODY(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
            size += obj_size_tree(
                unsafe { libr::CLOENV(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
        },
        PROMSXP => {
            size += obj_size_tree(
                unsafe { libr::PRVALUE(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
            size += obj_size_tree(
                unsafe { libr::PRCODE(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
            size += obj_size_tree(
                unsafe { libr::PRENV(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
        },
        EXTPTRSXP => {
            size += size_of::<*mut c_void>(); // the actual pointer
            size += obj_size_tree(
                unsafe { libr::EXTPTR_PROT(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
            size += obj_size_tree(
                unsafe { libr::EXTPTR_TAG(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
        },
        S4SXP => {
            size += obj_size_tree(
                unsafe { libr::TAG(x) },
                base_env,
                sizeof_node,
                sizeof_vector,
                seen,
                depth + 1,
            );
        },
        SYMSXP => {},
        _ => {},
    }
    size
}

fn is_linked_list(x: SEXP) -> bool {
    match r_typeof(x) {
        DOTSXP | LISTSXP | LANGSXP => true,
        _ => false,
    }
}

fn is_namespace(x: SEXP) -> bool {
    x == R_ENVS.base_ns ||
        unsafe {
            libr::Rf_findVarInFrame(x, r_symbol!(".__NAMESPACE__.")) != libr::R_UnboundValue
        }
}

fn v_size(n: usize, element_size: usize) -> usize {
    if n == 0 {
        return 0;
    }

    let vec_size = std::cmp::max(size_of::<SEXP>(), size_of::<c_double>()) as f64;
    let elements_per_byte = vec_size / element_size as f64;
    let n_bytes = (n as f64 / elements_per_byte).ceil() as usize;

    let mut size: usize = 0;
    if n_bytes > 16 {
        size = n_bytes * 8;
    } else if n_bytes > 8 {
        size = 128;
    } else if n_bytes > 6 {
        size = 64;
    } else if n_bytes > 4 {
        size = 48;
    } else if n_bytes > 2 {
        size = 32;
    } else if n_bytes > 1 {
        size = 16;
    } else if n_bytes > 0 {
        size = 8;
    }

    size
}

#[cfg(test)]
mod tests {
    use crate::size::r_size;
    use crate::fixtures::r_task;

    fn object_size(code: &str) -> usize {
        let object = harp::parse_eval_global(code).unwrap();
        r_size(object.sexp).unwrap()
    }

    fn expect_size(code: &str, expected: usize) {
        let size_act = object_size(code);
        assert_eq!(size_act, expected);
    }

    fn expect_same(code: &str) {
        let size_expected: f64 =
            harp::parse_eval_global(format!("utils::object.size({code})").as_str())
                .unwrap()
                .try_into()
                .unwrap();

        expect_size(code, size_expected as usize);
    }

    #[test]
    fn test_length_one_vectors() {
        r_task(|| {
            expect_same("1L");
            expect_same("'abc'");
            expect_same("paste(rep('banana', 100), collapse = '')");
            expect_same("charToRaw('a')");
            expect_same("5 + 1i");
        });
    }

    // size scales correctly with length (accounting for vector pool)
    #[test]
    fn test_sizes_scale_correctly() {
        r_task(|| {
            expect_same("numeric()");
            expect_same("1");
            expect_same("2");
            expect_same("c(1:10)");
            expect_same("c(1:1000)");
        });
    }

    #[test]
    fn test_size_of_lists() {
        r_task(|| {
            expect_same("list()");
            expect_same("as.list(1)");
            expect_same("as.list(1:2)");
            expect_same("as.list(1:3)");

            expect_same("list(list(list(list(list()))))");
        });
    }

    #[test]
    fn test_size_of_symbols() {
        r_task(|| {
            expect_same("quote(x)");
            expect_same("quote(asfsadfasdfasdfds)");
        });
    }

    #[test]
    fn test_pairlists() {
        r_task(|| {
            expect_same("pairlist()");
            expect_same("pairlist(1)");
            expect_same("pairlist(1, 2)");
            expect_same("pairlist(1, 2, 3)");
            expect_same("pairlist(1, 2, 3, 4)");
        });
    }

    #[test]
    fn test_s4_classes() {
        r_task(|| expect_same("methods::setClass('Z', slots = c(x = 'integer'))(x=1L)"));
    }

    #[test]
    fn test_size_attributes() {
        r_task(|| {
            expect_same("c(x = 1)");
            expect_same("list(x = 1)");
            expect_same("c(x = 'y')");
        });
    }

    #[test]
    fn test_duplicated_charsxps_counted_once() {
        r_task(|| {
            expect_same("'x'");
            expect_same("c('x', 'y', 'x')");
            expect_same("c('banana', 'banana', 'banana')");
        });
    }

    #[test]
    fn test_shared_components_once() {
        r_task(|| {
            let size1 = object_size(
                "local({
                x <- 1:1e3
                z <- list(x, x, x)})",
            );

            let size2 = object_size("1:1e3");
            let size3 = object_size("vector('list', 3)");

            assert_eq!(size1, size2 + size3)
        });
    }

    #[test]
    fn test_size_closures() {
        r_task(|| {
            let code = "local({
                f <- function() NULL
                attributes(f) <- NULL # zap srcrefs
                environment(f) <- emptyenv()
                f
            })";
            expect_same(code);
        });
    }

    #[test]
    fn test_works_for_altrep() {
        r_task(|| {
            let size = object_size("1:1e6");
            // Currently reported size is 640 B
            // If regular vector would be 4,000,040 B
            // This test is conservative so shouldn't fail in case representation
            // changes in the future
            assert!(size < 10000)
        });
    }

    #[test]
    fn test_compute_size_defered_strings() {
        r_task(|| {
            let code = "local({
                x <- 1:64
                names(x) <- x
                y <- names(x)
                y
            })";

            // Just don't crash
            object_size(code);
        });
    }

    #[test]
    fn test_terminal_envs_have_size_zero() {
        r_task(|| {
            expect_size("globalenv()", 0);
            expect_size("baseenv()", 0);
            expect_size("emptyenv()", 0);
            expect_size("asNamespace('stats')", 0);
        });
    }

    #[test]
    fn test_env_size_recursive() {
        r_task(|| {
            let e_size = object_size("new.env(parent = emptyenv())");

            let f_size = object_size(
                "local({
                e <- new.env(parent = emptyenv())
                f <- new.env(parent = e)
            })",
            );

            assert_eq!(f_size, 2 * e_size);
        });
    }

    #[test]
    fn test_size_of_functions_include_envs() {
        r_task(|| {
            let code = "local({
              f <- function() {
                y <- 1:1e3 + 1L
                a ~ b
              }
              f()
            })";

            assert!(object_size(code) > object_size("1:1e3 + 1L"));

            let code = "local({
                g <- function() {
                  y <- 1:1e3 + 1L
                  function() 10
                }
                g()
            })";

            assert!(object_size(code) > object_size("1:1e3 + 1L"));
        });
    }

    #[test]
    fn test_support_dots() {
        r_task(|| {
            // Check it doesn't error
            let size = object_size("(function(...) function() NULL)(foo)");
            assert!(size != 0)
        });
    }

    #[test]
    fn test_immediate_bindings() {
        r_task(|| {
            let size = object_size(
                "local({
                    f <- compiler::cmpfun(function() for (i in 1:3) return(environment()))
                    f()
                })",
            );
            assert!(size != 0)
        });
    }
}
