//
// environment.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::ops::Deref;

use libr::R_BaseEnv;
use libr::R_EmptyEnv;
use libr::R_GlobalEnv;
use libr::R_NilValue;
use libr::R_UnboundValue;
use libr::R_altrep_data1;
use libr::R_altrep_data2;
use libr::R_xlen_t;
use libr::Rf_defineVar;
use libr::Rf_findVarInFrame;
use libr::Rf_xlength;
use libr::ATTRIB;
use libr::CAR;
use libr::CDR;
use libr::ENVSXP;
use libr::EXPRSXP;
use libr::FRAME;
use libr::HASHTAB;
use libr::LANGSXP;
use libr::LISTSXP;
use libr::PRCODE;
use libr::PROMSXP;
use libr::PRVALUE;
use libr::SEXP;
use libr::SYMSXP;
use libr::TAG;
use libr::VECSXP;
use libr::VECTOR_ELT;
use once_cell::sync::Lazy;
use stdext::unwrap;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::RObject;
use crate::r_symbol;
use crate::symbol::RSymbol;
use crate::utils::r_is_altrep;
use crate::utils::r_is_null;
use crate::utils::r_is_s4;
use crate::utils::r_typeof;
use crate::utils::Sxpinfo;

pub struct REnvs {
    pub global: SEXP,
    pub base: SEXP,
    pub empty: SEXP,
}

// Silences diagnostics when called from `r_task()`. Should only be
// accessed from the R thread.
unsafe impl Send for REnvs {}
unsafe impl Sync for REnvs {}

pub static R_ENVS: Lazy<REnvs> = Lazy::new(|| unsafe {
    REnvs {
        global: R_GlobalEnv,
        base: R_BaseEnv,
        empty: R_EmptyEnv,
    }
});

#[derive(Eq)]
pub struct BindingReference {
    pub reference: bool,
}

fn has_reference(value: SEXP) -> bool {
    if r_is_null(value) {
        return false;
    }

    if r_is_altrep(value) {
        unsafe {
            return has_reference(R_altrep_data1(value)) || has_reference(R_altrep_data2(value));
        }
    }

    unsafe {
        // S4 slots are attributes and might be expandable
        // so we need to check if they have reference objects
        if r_is_s4(value) && has_reference(ATTRIB(value)) {
            return true;
        }
    }

    let rtype = r_typeof(value);
    match rtype {
        ENVSXP => true,

        LISTSXP | LANGSXP => unsafe { has_reference(CAR(value)) || has_reference(CDR(value)) },

        VECSXP | EXPRSXP => unsafe {
            let n = Rf_xlength(value);
            let mut has_ref = false;
            for i in 0..n {
                if has_reference(VECTOR_ELT(value, i)) {
                    has_ref = true;
                    break;
                }
            }
            has_ref
        },

        _ => false,
    }
}

impl BindingReference {
    fn new(value: SEXP) -> Self {
        Self {
            reference: has_reference(value),
        }
    }
}

impl PartialEq for BindingReference {
    fn eq(&self, other: &Self) -> bool {
        !(self.reference || other.reference)
    }
}

#[derive(Eq, PartialEq)]
pub enum BindingValue {
    Active {
        fun: RObject,
    },
    Promise {
        promise: RObject,
    },
    Altrep {
        object: RObject,
        data1: RObject,
        data2: RObject,
        reference: BindingReference,
    },
    Standard {
        object: RObject,
        reference: BindingReference,
    },
}

#[derive(Eq, PartialEq)]
pub struct Binding {
    pub name: RSymbol,
    pub value: BindingValue,
}

impl Binding {
    pub fn new(env: SEXP, frame: SEXP) -> Self {
        unsafe {
            let name = RSymbol::new_unchecked(TAG(frame));

            let info = Sxpinfo::interpret(&frame);

            if info.is_immediate() {
                // force the immediate bindings before we can safely call CAR()
                Rf_findVarInFrame(env, *name);
            }
            let mut value = CAR(frame);

            if info.is_active() {
                let value = BindingValue::Active {
                    fun: RObject::from(value),
                };
                return Self { name, value };
            }

            if r_typeof(value) == PROMSXP {
                let pr_value = PRVALUE(value);
                if pr_value == R_UnboundValue {
                    let code = PRCODE(value);
                    match r_typeof(code) {
                        // only consider calls and symbols to be promises
                        LANGSXP | SYMSXP => {
                            let value = BindingValue::Promise {
                                promise: RObject::from(value),
                            };
                            return Self { name, value };
                        },
                        // all other types are not regarded as promises
                        // but rather as their underlying object
                        _ => {
                            value = code;
                        },
                    }
                } else {
                    value = pr_value;
                }
            }

            if r_is_altrep(value) {
                let value = BindingValue::Altrep {
                    object: RObject::from(value),
                    data1: RObject::from(R_altrep_data1(value)),
                    data2: RObject::from(R_altrep_data2(value)),
                    reference: BindingReference::new(value),
                };
                return Self { name, value };
            }

            let value = BindingValue::Standard {
                object: RObject::from(value),
                reference: BindingReference::new(value),
            };
            Self { name, value }
        }
    }

    pub fn is_hidden(&self) -> bool {
        String::from(self.name).starts_with(".")
    }

    pub fn is_active(&self) -> bool {
        if let BindingValue::Active { .. } = self.value {
            true
        } else {
            false
        }
    }
}

pub struct Environment {
    env: RObject,
}

impl Deref for Environment {
    type Target = SEXP;
    fn deref(&self) -> &Self::Target {
        &self.env.sexp
    }
}

pub struct HashedEnvironmentIter<'a> {
    env: &'a Environment,

    hashtab: SEXP,
    hashtab_index: R_xlen_t,
    frame: SEXP,
}

impl<'a> HashedEnvironmentIter<'a> {
    pub fn new(env: &'a Environment) -> Self {
        unsafe {
            let hashtab = HASHTAB(**env);
            let hashtab_len = Rf_xlength(hashtab);
            let mut hashtab_index = 0;
            let mut frame = R_NilValue;

            // look for the first non null frame
            loop {
                let f = VECTOR_ELT(hashtab, hashtab_index);
                if f != R_NilValue {
                    frame = f;
                    break;
                }

                hashtab_index = hashtab_index + 1;
                if hashtab_index == hashtab_len {
                    break;
                }
            }

            Self {
                env,
                hashtab,
                hashtab_index,
                frame,
            }
        }
    }
}

impl<'a> Iterator for HashedEnvironmentIter<'a> {
    type Item = Binding;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            if self.frame == R_NilValue {
                return None;
            }

            // grab the next Binding
            let binding = Binding::new(*self.env.env, self.frame);

            // and advance to next binding
            self.frame = CDR(self.frame);

            if self.frame == R_NilValue {
                // end of frame: move to the next non empty frame
                let hashtab_len = Rf_xlength(self.hashtab);
                loop {
                    // move to the next frame
                    self.hashtab_index = self.hashtab_index + 1;

                    // end of iteration
                    if self.hashtab_index == hashtab_len {
                        self.frame = R_NilValue;
                        break;
                    }

                    // skip empty frames
                    self.frame = VECTOR_ELT(self.hashtab, self.hashtab_index);
                    if self.frame != R_NilValue {
                        break;
                    }
                }
            }

            Some(binding)
        }
    }
}

pub struct NonHashedEnvironmentIter<'a> {
    env: &'a Environment,

    frame: SEXP,
}

impl<'a> NonHashedEnvironmentIter<'a> {
    pub fn new(env: &'a Environment) -> Self {
        unsafe {
            Self {
                env,
                frame: FRAME(**env),
            }
        }
    }
}

impl<'a> Iterator for NonHashedEnvironmentIter<'a> {
    type Item = Binding;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            if self.frame == R_NilValue {
                None
            } else {
                let binding = Binding::new(*self.env.env, self.frame);
                self.frame = CDR(self.frame);
                Some(binding)
            }
        }
    }
}

pub enum EnvironmentIter<'a> {
    Hashed(HashedEnvironmentIter<'a>),
    NonHashed(NonHashedEnvironmentIter<'a>),
}

impl<'a> EnvironmentIter<'a> {
    pub fn new(env: &'a Environment) -> Self {
        unsafe {
            let hashtab = HASHTAB(**env);
            if hashtab == R_NilValue {
                EnvironmentIter::NonHashed(NonHashedEnvironmentIter::new(env))
            } else {
                EnvironmentIter::Hashed(HashedEnvironmentIter::new(env))
            }
        }
    }
}

pub enum EnvironmentFilter {
    IncludeHiddenBindings,
    ExcludeHiddenBindings,
}

impl<'a> Iterator for EnvironmentIter<'a> {
    type Item = Binding;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            EnvironmentIter::Hashed(iter) => iter.next(),
            EnvironmentIter::NonHashed(iter) => iter.next(),
        }
    }
}

impl Environment {
    pub fn new(env: RObject) -> Self {
        Self { env }
    }

    pub fn bind(&self, name: &str, value: impl Into<SEXP>) {
        unsafe {
            Rf_defineVar(r_symbol!(name), value.into(), self.env.sexp);
        }
    }

    pub fn iter(&self) -> EnvironmentIter {
        EnvironmentIter::new(&self)
    }

    pub fn exists(&self, name: impl Into<RSymbol>) -> bool {
        let name = name.into();
        match self.iter() {
            EnvironmentIter::Hashed(mut iter) => {
                /* TODO: we could eventually only iterate in the frame with the right hash

                // does the symbol know its hash:
                let has_hash = name.has_hash();

                hashcode = HASHVALUE(PRINTNAME(TAG(frame))) % HASHSIZE(table)
                chain = VECTOR_ELT(table, hashcode);

                */
                iter.find(|b| b.name == name).is_some()
            },
            EnvironmentIter::NonHashed(mut iter) => iter.find(|b| b.name == name).is_some(),
        }
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

#[cfg(test)]
mod tests {
    use libr::Rf_ScalarInteger;
    use libr::Rf_defineVar;

    use super::*;
    use crate::exec::RFunction;
    use crate::exec::RFunctionExt;
    use crate::r_symbol;
    use crate::r_test;

    unsafe fn test_environment_iter_impl(hash: bool) {
        let test_env = RFunction::new("base", "new.env")
            .param("parent", R_EmptyEnv)
            .param("hash", RObject::from(hash))
            .call()
            .unwrap();

        let sym = r_symbol!("a");
        Rf_defineVar(sym, Rf_ScalarInteger(42), test_env.sexp);

        let sym = r_symbol!("b");
        Rf_defineVar(sym, Rf_ScalarInteger(43), test_env.sexp);

        let sym = r_symbol!("c");
        Rf_defineVar(sym, Rf_ScalarInteger(44), test_env.sexp);

        let env = Environment::new(test_env);
        assert_eq!(env.iter().count(), 3);
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_environment_iter() {
        r_test! {
            test_environment_iter_impl(true);
            test_environment_iter_impl(false);
        }
    }
}
