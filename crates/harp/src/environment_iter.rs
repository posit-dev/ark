use libr::*;

use crate::environment::Environment;
use crate::object::RObject;
use crate::r_is_altrep;
use crate::r_typeof;
use crate::symbol::RSymbol;

pub struct EnvironmentIter {
    env: Environment,
    names: std::vec::IntoIter<String>,
}

pub struct Binding {
    pub name: RSymbol,
    pub value: BindingValue,
}

#[derive(Eq, PartialEq)]
pub enum RObjectValueId {
    Active(SEXP),
    Promise(SEXP),
    Altrep(SEXP, SEXP, SEXP),
    Standard(SEXP),
}

pub enum BindingValue {
    Active {
        fun: RObject,
    },
    Promise {
        promise: RObject,
    },
    Altrep {
        object: RObject,
        // Unprotected pointers, only compared by identity in `id()` to detect
        // changes to the ALTREP's internal state. Protecting them would
        // increase their reference count, and R >= 4.6.2 errors when the
        // data of a locked wrapper object is shared.
        // https://github.com/posit-dev/positron/issues/17279
        data1: SEXP,
        data2: SEXP,
    },
    Standard {
        object: RObject,
    },
}

impl BindingValue {
    // Use id() to compare binding values by their pointers.
    pub fn id(&self) -> RObjectValueId {
        match self {
            BindingValue::Active { fun } => RObjectValueId::Active(fun.sexp),
            BindingValue::Promise { promise } => RObjectValueId::Promise(promise.sexp),
            BindingValue::Altrep {
                object,
                data1,
                data2,
            } => RObjectValueId::Altrep(object.sexp, *data1, *data2),
            BindingValue::Standard { object } => RObjectValueId::Standard(object.sexp),
        }
    }
}

impl EnvironmentIter {
    pub fn new(env: Environment) -> Self {
        let names = env.names().into_iter();
        Self { env, names }
    }
}

impl Iterator for EnvironmentIter {
    type Item = harp::Result<Binding>;

    fn next(&mut self) -> Option<Self::Item> {
        self.names
            .next()
            .map(|name| Binding::new(&self.env, (&name).into()))
    }
}

impl Binding {
    pub fn new(env: &Environment, name: RSymbol) -> harp::Result<Self> {
        unsafe {
            if env.is_active(name)? {
                let fun = libr::R_ActiveBindingFunction(name.sexp, env.inner.sexp);
                let value = BindingValue::Active {
                    fun: RObject::from(fun),
                };
                return Ok(Self { name, value });
            };

            let value = env.find(name)?;

            if r_is_altrep(value) {
                let value = BindingValue::Altrep {
                    object: RObject::from(value),
                    data1: R_altrep_data1(value),
                    data2: R_altrep_data2(value),
                };
                return Ok(Self { name, value });
            }

            if r_typeof(value) == PROMSXP {
                let pr_value = PRVALUE(value);
                if pr_value != R_UnboundValue {
                    // Forced promise
                    return Self::new_standard(name, pr_value);
                }

                let code = PRCODE(value);

                if let LANGSXP | SYMSXP = r_typeof(code) {
                    // Promise to a symbolic expression
                    let value = BindingValue::Promise {
                        promise: RObject::from(value),
                    };
                    return Ok(Self { name, value });
                }

                // Promise to a literal expression
                return Self::new_standard(name, code);
            }

            Self::new_standard(name, value)
        }
    }

    fn new_standard(name: RSymbol, value: SEXP) -> harp::Result<Self> {
        let value = BindingValue::Standard {
            object: RObject::from(value),
        };
        Ok(Self { name, value })
    }

    pub fn is_hidden(&self) -> bool {
        String::from(self.name).starts_with(".")
    }

    pub fn is_active(&self) -> bool {
        matches!(self.value, BindingValue::Active { .. })
    }

    // Use id() to compare bindings by their pointers.
    pub fn id(&self) -> (SEXP, RObjectValueId) {
        (self.name.sexp, self.value.id())
    }
}
#[cfg(test)]
mod tests {
    use libr::Rf_ScalarInteger;
    use libr::Rf_defineVar;

    use super::*;
    use crate::exec::RFunction;
    use crate::exec::RFunctionExt;
    use crate::fixtures::r_task;
    use crate::r_symbol;
    use crate::R_ENVS;

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
        r_task(|| unsafe {
            test_environment_iter_impl(true);
            test_environment_iter_impl(false);
        })
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_binding_eq() {
        r_task(|| {
            let env: Environment = Environment::new_empty();

            let obj = harp::parse_eval_base("1").unwrap();
            env.bind(RSymbol::from("a"), &obj);
            env.bind(RSymbol::from("b"), &obj);

            let mut iter = env.iter();
            let a = iter.next().unwrap().unwrap();
            let b = iter.next().unwrap().unwrap();

            // same object bound to different symbols
            assert!(!(a.id() == b.id()));
            assert!(a.value.id() == b.value.id());

            // now bind a different object to b
            let b = harp::parse_eval_base("1").unwrap();
            env.bind(RSymbol::from("b"), &b);

            let mut iter = env.iter();
            let a = iter.next().unwrap().unwrap();
            let b = iter.next().unwrap().unwrap();
            // Even though they are equal by value, their id() is different
            assert!(!(a.value.id() == b.value.id()));
        })
    }

    // https://github.com/posit-dev/positron/issues/17279
    #[test]
    fn test_binding_does_not_share_altrep_data() {
        r_task(|| {
            // `drop()` on a referenced vector of length >= 64 returns an
            // ALTREP wrapper
            harp::parse_eval_global("x <- drop(matrix(as.double(1:64), ncol = 1))").unwrap();

            let binding = Binding::new(&Environment::view(R_ENVS.global), "x".into()).unwrap();
            assert!(matches!(binding.value, BindingValue::Altrep { .. }));

            // The first use locks the wrapper's data. If the binding holds a
            // reference to that data, R >= 4.6.2 fails on the second use with
            // "REFCNT on locked WRAPPER data increased to 2".
            harp::parse_eval_global("x + 1").unwrap();
            let binding = Binding::new(&Environment::view(R_ENVS.global), "x".into()).unwrap();
            harp::parse_eval_global("x + 2").unwrap();

            drop(binding);
        })
    }
}
