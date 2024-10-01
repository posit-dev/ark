//
// environment.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//
use libr::*;
use once_cell::sync::Lazy;
use stdext::unwrap;

pub use crate::environment_iter::*;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::RObject;
use crate::r_env_binding_is_active;
use crate::symbol::RSymbol;

const FRAME_LOCK_MASK: std::ffi::c_int = 1 << 14;

#[derive(Clone)]
pub struct Environment {
    pub inner: RObject,
    filter: EnvironmentFilter,
}

#[derive(Clone)]
pub enum EnvironmentFilter {
    None,
    ExcludeHidden,
}

impl Default for EnvironmentFilter {
    fn default() -> Self {
        Self::None
    }
}

pub struct REnvs {
    pub global: SEXP,
    pub base: SEXP,
    pub base_ns: SEXP,
    pub empty: SEXP,
}

pub static R_ENVS: Lazy<REnvs> = Lazy::new(|| unsafe {
    REnvs {
        global: R_GlobalEnv,
        base: R_BaseEnv,
        base_ns: R_BaseNamespace,
        empty: R_EmptyEnv,
    }
});

impl Environment {
    pub fn new(env: RObject) -> Self {
        Self::new_filtered(env, EnvironmentFilter::default())
    }

    pub fn new_filtered(env: RObject, filter: EnvironmentFilter) -> Self {
        Self { inner: env, filter }
    }

    pub fn view(env: SEXP) -> Self {
        Self {
            inner: RObject::view(env),
            filter: EnvironmentFilter::default(),
        }
    }

    pub fn parent(&self) -> Option<Environment> {
        unsafe {
            let parent = ENCLOS(self.inner.sexp);
            if parent == R_ENVS.empty {
                None
            } else {
                Some(Self::new_filtered(
                    RObject::new(parent),
                    self.filter.clone(),
                ))
            }
        }
    }

    pub fn ancestors(&self) -> impl Iterator<Item = Environment> {
        std::iter::successors(Some(self.clone()), |p| p.parent())
    }

    pub fn bind(&self, name: RSymbol, value: impl Into<SEXP>) {
        unsafe {
            Rf_defineVar(name.sexp, value.into(), self.inner.sexp);
        }
    }

    pub fn force_bind(&self, name: RSymbol, value: impl Into<SEXP>) {
        let locked = self.is_locked_binding(name);
        if locked {
            self.unlock_binding(name);
        }

        self.bind(name, value);

        if locked {
            self.lock_binding(name);
        }
    }

    pub fn iter(&self) -> EnvironmentIter {
        EnvironmentIter::new(self.clone())
    }

    pub fn exists(&self, name: impl Into<RSymbol>) -> bool {
        unsafe { libr::R_existsVarInFrame(self.inner.sexp, name.into().sexp) != 0 }
    }

    pub fn find(&self, name: impl Into<RSymbol>) -> harp::Result<SEXP> {
        let name = name.into();
        unsafe {
            let out = Rf_findVarInFrame(self.inner.sexp, *name);

            if out == R_UnboundValue {
                Err(harp::Error::MissingBindingError { name: name.into() })
            } else {
                Ok(out)
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        match self.filter {
            EnvironmentFilter::None => self.inner.length() == 0,
            EnvironmentFilter::ExcludeHidden => self
                .iter()
                .filter_map(|b| b.ok())
                .filter(|b| !b.is_hidden())
                .next()
                .is_none(),
        }
    }

    pub fn length(&self) -> usize {
        match self.filter {
            EnvironmentFilter::None => self.inner.length() as usize,
            EnvironmentFilter::ExcludeHidden => self
                .iter()
                .filter_map(|b| b.ok())
                .filter(|b| !b.is_hidden())
                .count(),
        }
    }

    /// Returns environment name if it has one. Reproduces the same output as
    /// `rlang::env_name()`.
    pub fn name(&self) -> Option<String> {
        let name = RFunction::new("", ".ps.env_name")
            .add(self.inner.sexp)
            .call();
        let name = unwrap!(name, Err(err) => {
            log::error!("{err:?}");
            return None
        });

        if unsafe { name.sexp == R_NilValue } {
            return None;
        }

        let name: Result<String, crate::error::Error> = name.try_into();
        let name = unwrap!(name, Err(err) => {
            log::error!("{err:?}");
            return None;
        });

        Some(name)
    }

    /// Returns the names of the bindings of the environment
    pub fn names(&self) -> Vec<String> {
        let all_names = match self.filter {
            EnvironmentFilter::None => Rboolean_TRUE,
            EnvironmentFilter::ExcludeHidden => Rboolean_FALSE,
        };

        // `all = all_names`, `sorted = false`
        // We don't sort the elements when fetchhing from R, but sort them
        // later in Rust
        let names =
            RObject::from(unsafe { R_lsInternal3(self.inner.sexp, all_names, Rboolean_FALSE) });
        let mut names = Vec::<String>::try_from(names).unwrap_or(Vec::new());
        names.sort();

        names
    }

    pub fn lock(&self, bindings: bool) {
        unsafe {
            libr::R_LockEnvironment(self.inner.sexp, bindings.into());
        }
    }

    pub fn unlock(&self) {
        let unlocked_mask = self.flags() & !FRAME_LOCK_MASK;
        unsafe { libr::SET_ENVFLAGS(self.inner.sexp, unlocked_mask) }
    }

    pub fn lock_binding(&self, name: RSymbol) {
        unsafe {
            libr::R_LockBinding(name.sexp, self.inner.sexp);
        }
    }

    pub fn unlock_binding(&self, name: RSymbol) {
        unsafe {
            libr::R_unLockBinding(name.sexp, self.inner.sexp);
        }
    }

    pub fn is_locked(&self) -> bool {
        unsafe { libr::R_EnvironmentIsLocked(self.inner.sexp) != 0 }
    }

    pub fn is_locked_binding(&self, name: RSymbol) -> bool {
        unsafe { libr::R_BindingIsLocked(name.sexp, self.inner.sexp) != 0 }
    }

    pub fn is_active(&self, name: RSymbol) -> harp::Result<bool> {
        r_env_binding_is_active(self.inner.sexp, name.sexp)
    }

    fn flags(&self) -> std::ffi::c_int {
        unsafe { libr::ENVFLAGS(self.inner.sexp) }
    }

    pub fn as_list(&self) -> harp::Result<RObject> {
        RFunction::new("", "as.list")
            .add(self.inner.sexp)
            // TODO: Respect the future hidden flag
            .param("all.names", true)
            .call_in(R_ENVS.base)
    }
}

impl From<Environment> for SEXP {
    fn from(object: Environment) -> Self {
        object.inner.sexp
    }
}

impl From<Environment> for RObject {
    fn from(object: Environment) -> Self {
        object.inner
    }
}

// Silences diagnostics when called from `r_task()`. Should only be
// accessed from the R thread.
unsafe impl Send for REnvs {}
unsafe impl Sync for REnvs {}

#[harp::register]
pub extern "C" fn ark_env_unlock(env: SEXP) -> crate::error::Result<SEXP> {
    unsafe {
        crate::check_env(env)?;
        Environment::view(env).unlock();
        Ok(libr::R_NilValue)
    }
}

pub fn r_ns_env(name: &str) -> anyhow::Result<Environment> {
    let registry = Environment::new(unsafe { R_NamespaceRegistry.into() });
    let ns = registry.find(name)?;

    Ok(Environment::new(ns.into()))
}

#[cfg(test)]
mod tests {
    use libr::Rf_ScalarInteger;
    use libr::Rf_defineVar;

    use super::*;
    use crate::exec::RFunction;
    use crate::exec::RFunctionExt;
    use crate::object::r_length;
    use crate::r_symbol;

    fn new_test_environment(hash: bool) -> Environment {
        let test_env = RFunction::new("base", "new.env")
            .param("parent", R_ENVS.empty)
            .param("hash", RObject::from(hash))
            .call()
            .unwrap();

        unsafe {
            let sym = r_symbol!("a");
            Rf_defineVar(sym, Rf_ScalarInteger(42), test_env.sexp);

            let sym = r_symbol!("b");
            Rf_defineVar(sym, Rf_ScalarInteger(43), test_env.sexp);

            let sym = r_symbol!("c");
            Rf_defineVar(sym, Rf_ScalarInteger(44), test_env.sexp);
        }

        Environment::new(test_env)
    }

    #[test]
    fn test_environment_iter_count() {
        crate::r_task(|| {
            let hashed = new_test_environment(true);
            let non_hashed = new_test_environment(false);
            assert_eq!(hashed.iter().count(), 3);
            assert_eq!(non_hashed.iter().count(), 3);

            let base = Environment::new(R_ENVS.base_ns.into());
            let n_base = r_length(R_ENVS.base_ns) as usize;
            assert_eq!(base.iter().count(), n_base);
        })
    }

    #[test]
    fn test_sorted_environment_names() {
        crate::r_task(|| {
            let env = harp::parse_eval_global("as.environment(list(c = 1, b = 2, a = 3))").unwrap();
            let names = Environment::new(env.clone()).names();
            assert_eq!(names, vec!["a", "b", "c"]);

            // Also assert that `.iter()` will be sorted
            let expected = vec!["a", "b", "c"];
            for (i, binding) in Environment::new(env).iter().enumerate() {
                assert_eq!(binding.unwrap().name, expected[i]);
            }
        })
    }

    #[test]
    fn test_environments_with_classes() {
        // Here we are testing that environments iterators are not dispatching to
        // S3 methods that might have been implemented for `length()` or `names()`
        // which could cause issues if their len() didn't match.
        // https://github.com/posit-dev/positron/issues/3229
        crate::r_task(|| {
            let test_env: RObject = harp::parse_eval_global("new.env()").unwrap();
            let env = harp::parse_eval0(
                r#"
            x <- structure(new.env(), class = "test_env")
            names.test_env <- function(x) letters[1:3]
            length.test_env <- function(x) 3
            x
            "#,
                test_env.sexp,
            )
            .unwrap();

            let env: Environment = Environment::new(env);

            assert_eq!(env.names().len(), 0);
            assert_eq!(env.is_empty(), true);
            assert_eq!(env.length(), 0);

            // Make sure that it would actually dispatch to the s3 methods we implemented
            let names: Vec<String> = harp::parse_eval0("names(x)", test_env.sexp)
                .unwrap()
                .try_into()
                .unwrap();
            assert_eq!(names.len(), 3);

            let len: i32 = harp::parse_eval0("length(x)", test_env.sexp)
                .unwrap()
                .try_into()
                .unwrap();
            assert_eq!(len, 3);
        })
    }

    #[test]
    fn test_filtered_env() {
        crate::r_task(|| {
            let env =
                harp::parse_eval_global("as.environment(list(.a = 1, b = 2, c = 3))").unwrap();
            let env = Environment::new_filtered(env, EnvironmentFilter::ExcludeHidden);
            assert_eq!(env.length(), 2);
            assert_eq!(env.names(), vec!["b", "c"]);

            // Make sure the iterator is also filtered
            assert_eq!(env.iter().count(), 2);
        })
    }
}
