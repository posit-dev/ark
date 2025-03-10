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

#[derive(Eq)]
pub struct Binding {
    pub name: RSymbol,
    pub value: BindingValue,
}

// Bindings are equal if their names and values are exactly the same.
impl PartialEq for Binding {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.value == other.value
    }
}

#[derive(Eq)]
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
    },
    Standard {
        object: RObject,
    },
}

// Two binding values are equal if they are identical (ie, their SEXP's)
// are the same.
impl PartialEq for BindingValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Active { fun: a }, Self::Active { fun: b }) => a.sexp == b.sexp,
            (Self::Promise { promise: a }, Self::Promise { promise: b }) => a.sexp == b.sexp,
            (
                Self::Altrep {
                    object: a,
                    data1: b,
                    data2: c,
                    ..
                },
                Self::Altrep {
                    object: d,
                    data1: e,
                    data2: f,
                    ..
                },
            ) => a.sexp == d.sexp && b.sexp == e.sexp && c.sexp == f.sexp,
            (Self::Standard { object: a, .. }, Self::Standard { object: b, .. }) => {
                a.sexp == b.sexp
            },
            _ => false,
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
                    data1: RObject::from(R_altrep_data1(value)),
                    data2: RObject::from(R_altrep_data2(value)),
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
        if let BindingValue::Active { .. } = self.value {
            true
        } else {
            false
        }
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
            let env: Environment = Environment::new_empty().unwrap();

            let obj = harp::parse_eval_base("1").unwrap();
            env.bind(RSymbol::from("a"), &obj);
            env.bind(RSymbol::from("b"), &obj);

            let mut iter = env.iter();
            let a = iter.next().unwrap().unwrap();
            let b = iter.next().unwrap().unwrap();

            assert_eq!(a == b, false);
            assert_eq!(a.name == b.name, false);
            assert_eq!(a.value == b.value, true);
        })
    }
}
