use libr::SEXP;

// --- Closure accessors ---

pub fn fn_formals(x: SEXP) -> SEXP {
    unsafe { libr::FORMALS(x) }
}

pub fn fn_body(x: SEXP) -> SEXP {
    unsafe {
        if libr::has::R_ClosureBody() {
            libr::R_ClosureBody(x)
        } else {
            libr::BODY(x)
        }
    }
}

pub fn fn_env(x: SEXP) -> SEXP {
    unsafe {
        if libr::has::R_ClosureEnv() {
            libr::R_ClosureEnv(x)
        } else {
            libr::CLOENV(x)
        }
    }
}

pub fn new_function(formals: SEXP, body: SEXP, env: SEXP) -> SEXP {
    unsafe {
        if libr::has::R_mkClosure() {
            libr::R_mkClosure(formals, body, env)
        } else {
            let out = libr::Rf_allocSExp(libr::CLOSXP);
            libr::SET_FORMALS(out, formals);
            libr::SET_BODY(out, body);
            libr::SET_CLOENV(out, env);
            out
        }
    }
}

// --- Environment bindings ---

pub fn env_binding_is_locked(env: SEXP, sym: SEXP) -> bool {
    unsafe { libr::R_BindingIsLocked(sym, env) != 0 }
}

pub fn env_binding_lock(env: SEXP, sym: SEXP) {
    unsafe {
        libr::R_LockBinding(sym, env);
    }
}

pub fn env_binding_unlock(env: SEXP, sym: SEXP) {
    unsafe {
        libr::R_unLockBinding(sym, env);
    }
}

/// Binds a value in an environment, temporarily unlocking the binding if needed.
pub fn env_bind_force(env: SEXP, sym: SEXP, value: SEXP) {
    let locked = env_binding_is_locked(env, sym);
    if locked {
        env_binding_unlock(env, sym);
    }
    unsafe {
        libr::Rf_defineVar(sym, value, env);
    }
    if locked {
        env_binding_lock(env, sym);
    }
}

/// Returns the parent (enclosing) environment.
pub fn env_parent(env: SEXP) -> SEXP {
    unsafe {
        if libr::has::R_ParentEnv() {
            libr::R_ParentEnv(env)
        } else {
            libr::ENCLOS(env)
        }
    }
}

// --- Attributes ---

/// Gets an attribute from `x`.
pub fn attrib_get(x: SEXP, tag: SEXP) -> SEXP {
    unsafe { libr::Rf_getAttrib(x, tag) }
}

pub fn attrib_poke(x: SEXP, tag: SEXP, value: SEXP) {
    unsafe {
        libr::Rf_setAttrib(x, tag, value);
    }
}

/// Returns `true` if `x` has any attributes.
pub fn attrib_has_any(x: SEXP) -> bool {
    unsafe {
        if libr::has::ANY_ATTRIB() {
            libr::ANY_ATTRIB(x) != 0
        } else {
            libr::ATTRIB(x) != libr::R_NilValue
        }
    }
}

/// Iterates over the attributes of `x`, calling `f(tag, value)` for each.
pub fn attrib_for_each<F: FnMut(SEXP, SEXP)>(x: SEXP, mut f: F) {
    unsafe {
        if libr::has::R_mapAttrib() {
            unsafe extern "C-unwind" fn trampoline<F: FnMut(SEXP, SEXP)>(
                tag: SEXP,
                val: SEXP,
                data: *mut std::ffi::c_void,
            ) -> SEXP {
                let f = &mut *(data as *mut F);
                f(tag, val);
                SEXP::null()
            }
            let data = &mut f as *mut F as *mut std::ffi::c_void;
            libr::R_mapAttrib(x, Some(trampoline::<F>), data);
        } else {
            pub unsafe fn map_attrib<F: FnMut(SEXP, SEXP)>(x: SEXP, f: &mut F) {
                let mut node = libr::ATTRIB(x);
                while node != libr::R_NilValue {
                    f(libr::TAG(node), libr::CAR(node));
                    node = libr::CDR(node);
                }
            }
            map_attrib(x, &mut f);
        }
    }
}

/// Shallow-copies all attributes from `src` to `dst`.
pub fn attrib_poke_from(dst: SEXP, src: SEXP) {
    unsafe {
        libr::SHALLOW_DUPLICATE_ATTRIB(dst, src);
    }
}
