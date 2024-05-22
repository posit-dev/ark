//
// environment.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::ops::Deref;

use libr::*;
use once_cell::sync::Lazy;
use stdext::unwrap;

pub use crate::environment_iter::*;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::RObject;
use crate::r_symbol;
use crate::symbol::RSymbol;

const FRAME_LOCK_MASK: std::ffi::c_int = 1 << 14;

#[derive(Clone)]
pub struct Environment {
    pub env: RObject,
}

pub enum EnvironmentFilter {
    IncludeHiddenBindings,
    ExcludeHiddenBindings,
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
    pub fn new(env: RObject) -> Self {
        Self { env }
    }

    pub fn view(env: SEXP) -> Self {
        Self {
            env: RObject::view(env),
        }
    }

    pub fn parent(&self) -> Option<Environment> {
        unsafe {
            let parent = ENCLOS(self.env.sexp);
            if parent == R_ENVS.empty {
                None
            } else {
                Some(Self::new(RObject::new(parent)))
            }
        }
    }

    pub fn ancestors(&self) -> impl Iterator<Item = Environment> {
        std::iter::successors(Some(self.clone()), |p| p.parent())
    }

    pub fn bind(&self, name: &str, value: impl Into<SEXP>) {
        unsafe {
            Rf_defineVar(r_symbol!(name), value.into(), self.env.sexp);
        }
    }

    pub fn iter(&self) -> EnvironmentIter {
        EnvironmentIter::new(self.clone())
    }

    pub fn exists(&self, name: impl Into<RSymbol>) -> bool {
        unsafe { libr::R_existsVarInFrame(self.env.sexp, name.into().sexp) != 0 }
    }

    pub fn find(&self, name: impl Into<RSymbol>) -> SEXP {
        let name = name.into();
        unsafe { Rf_findVarInFrame(self.env.sexp, *name) }
    }

    pub fn is_empty(&self, filter: EnvironmentFilter) -> bool {
        match filter {
            EnvironmentFilter::IncludeHiddenBindings => self.env.length() == 0,
            EnvironmentFilter::ExcludeHiddenBindings => {
                self.iter().filter(|b| !b.is_hidden()).next().is_none()
            },
        }
    }

    pub fn length(&self, filter: EnvironmentFilter) -> usize {
        match filter {
            EnvironmentFilter::IncludeHiddenBindings => self.env.length() as usize,
            EnvironmentFilter::ExcludeHiddenBindings => {
                self.iter().filter(|b| !b.is_hidden()).count()
            },
        }
    }

    /// Returns environment name if it has one. Reproduces the same output as
    /// `rlang::env_name()`.
    pub fn name(&self) -> Option<String> {
        let name = RFunction::new("", ".ps.env_name").add(self.env.sexp).call();
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
        let names = RFunction::new("base", "names").add(self.env.sexp).call();
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
            libr::R_LockEnvironment(self.env.sexp, bindings.into());
        }
    }

    pub fn unlock(&mut self) {
        let unlocked_mask = self.flags() & !FRAME_LOCK_MASK;
        unsafe { libr::SET_ENVFLAGS(self.env.sexp, unlocked_mask) }
    }

    pub fn is_locked(&self) -> bool {
        unsafe { libr::R_EnvironmentIsLocked(self.env.sexp) != 0 }
    }

    pub fn is_active(&self, name: RSymbol) -> bool {
        unsafe { libr::R_BindingIsActive(name.sexp, self.env.sexp) != 0 }
    }

    fn flags(&self) -> std::ffi::c_int {
        unsafe { libr::ENVFLAGS(self.env.sexp) }
    }
}

impl Deref for Environment {
    type Target = SEXP;
    fn deref(&self) -> &Self::Target {
        &self.env.sexp
    }
}

impl From<Environment> for SEXP {
    fn from(object: Environment) -> Self {
        object.env.sexp
    }
}

impl From<Environment> for RObject {
    fn from(object: Environment) -> Self {
        object.env
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
