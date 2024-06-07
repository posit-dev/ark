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
use crate::r_symbol;
use crate::symbol::RSymbol;

const FRAME_LOCK_MASK: std::ffi::c_int = 1 << 14;

#[derive(Clone)]
pub struct Environment {
    pub inner: RObject,
    filter: EnvironmentFilter,
}

#[derive(Clone)]
pub enum EnvironmentFilter {
    IncludeHiddenBindings,
    ExcludeHiddenBindings,
}

impl Default for EnvironmentFilter {
    fn default() -> Self {
        Self::ExcludeHiddenBindings
    }
}

pub struct REnvs {
    pub global: SEXP,
    pub base: SEXP,
    pub empty: SEXP,
}

pub static R_ENVS: Lazy<REnvs> = Lazy::new(|| unsafe {
    REnvs {
        global: R_GlobalEnv,
        base: R_BaseEnv,
        empty: R_EmptyEnv,
    }
});

impl Environment {
    pub fn new(env: RObject, filter: EnvironmentFilter) -> Self {
        Self { inner: env, filter }
    }

    pub fn view(env: SEXP, filter: EnvironmentFilter) -> Self {
        Self {
            inner: RObject::view(env),
            filter,
        }
    }

    pub fn parent(&self) -> Option<Environment> {
        unsafe {
            let parent = ENCLOS(self.inner.sexp);
            if parent == R_ENVS.empty {
                None
            } else {
                Some(Self::new(RObject::new(parent), self.filter.clone()))
            }
        }
    }

    pub fn ancestors(&self) -> impl Iterator<Item = Environment> {
        std::iter::successors(Some(self.clone()), |p| p.parent())
    }

    pub fn bind(&self, name: &str, value: impl Into<SEXP>) {
        unsafe {
            Rf_defineVar(r_symbol!(name), value.into(), self.inner.sexp);
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
            EnvironmentFilter::IncludeHiddenBindings => self.inner.length() == 0,
            EnvironmentFilter::ExcludeHiddenBindings => self
                .iter()
                .filter_map(|b| b.ok())
                .filter(|b| !b.is_hidden())
                .next()
                .is_none(),
        }
    }

    pub fn length(&self) -> usize {
        match self.filter {
            EnvironmentFilter::IncludeHiddenBindings => self.inner.length() as usize,
            EnvironmentFilter::ExcludeHiddenBindings => self
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
            EnvironmentFilter::IncludeHiddenBindings => true,
            EnvironmentFilter::ExcludeHiddenBindings => false,
        };

        let names = RFunction::new("base", "ls")
            .param("envir", self.inner.sexp)
            .param("all.names", all_names)
            .call();

        let names = unwrap!(names, Err(err) => {
            log::error!("{err:?}");
            return vec![]
        });

        let names: Result<Vec<String>, crate::error::Error> = names.try_into();
        let names = unwrap!(names, Err(err) => {
            log::error!("{err:?}");
            return vec![];
        });

        names
    }

    pub fn lock(&mut self, bindings: bool) {
        unsafe {
            libr::R_LockEnvironment(self.inner.sexp, bindings.into());
        }
    }

    pub fn unlock(&mut self) {
        let unlocked_mask = self.flags() & !FRAME_LOCK_MASK;
        unsafe { libr::SET_ENVFLAGS(self.inner.sexp, unlocked_mask) }
    }

    pub fn is_locked(&self) -> bool {
        unsafe { libr::R_EnvironmentIsLocked(self.inner.sexp) != 0 }
    }

    pub fn is_active(&self, name: RSymbol) -> harp::Result<bool> {
        r_env_binding_is_active(self.inner.sexp, name.sexp)
    }

    fn flags(&self) -> std::ffi::c_int {
        unsafe { libr::ENVFLAGS(self.inner.sexp) }
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
        Environment::view(env, EnvironmentFilter::default()).unlock();
        Ok(libr::R_NilValue)
    }
}
