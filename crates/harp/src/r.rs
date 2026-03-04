use libr::SEXP;

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

/// Creates a closure.
pub unsafe fn new_function(formals: SEXP, body: SEXP, env: SEXP) -> SEXP {
    if libr::has::R_mkClosure() {
        libr::R_mkClosure(formals, body, env)
    } else {
        compat::alloc_closure(formals, body, env)
    }
}

mod compat {
    use libr::SEXP;

    pub unsafe fn alloc_closure(formals: SEXP, body: SEXP, env: SEXP) -> SEXP {
        let out = libr::Rf_allocSExp(libr::CLOSXP);
        libr::SET_FORMALS(out, formals);
        libr::SET_BODY(out, body);
        libr::SET_CLOENV(out, env);
        out
    }
}
