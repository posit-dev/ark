use libr::*;

use crate::environment::Environment;
use crate::object::RObject;
use crate::r_is_altrep;
use crate::r_is_null;
use crate::r_is_s4;
use crate::r_typeof;
use crate::symbol::RSymbol;

pub struct EnvironmentIter {
    env: Environment,
    names: std::vec::IntoIter<String>,
}

#[derive(Eq, PartialEq)]
pub struct Binding {
    pub name: RSymbol,
    pub value: BindingValue,
}

// What is this used for? Do we still need it?
// https://github.com/posit-dev/amalthea/commit/04a9fe20a48cb01c4fe2d6fe1a4dfdc0f2a186fc
#[derive(Eq)]
pub struct BindingNestedEnvironment {
    pub has_nested_environment: bool,
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
        has_nested_environment: BindingNestedEnvironment,
    },
    Standard {
        object: RObject,
        has_nested_environment: BindingNestedEnvironment,
    },
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

            if libr::ALTREP(value) != 0 {
                let value = BindingValue::Altrep {
                    object: RObject::from(value),
                    data1: RObject::from(R_altrep_data1(value)),
                    data2: RObject::from(R_altrep_data2(value)),
                    has_nested_environment: BindingNestedEnvironment::new(value),
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
            has_nested_environment: BindingNestedEnvironment::new(value),
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

impl BindingNestedEnvironment {
    fn new(value: SEXP) -> Self {
        Self {
            has_nested_environment: has_nested_environment(value),
        }
    }
}

impl PartialEq for BindingNestedEnvironment {
    fn eq(&self, other: &Self) -> bool {
        !(self.has_nested_environment || other.has_nested_environment)
    }
}

fn has_nested_environment(value: SEXP) -> bool {
    if r_is_null(value) {
        return false;
    }

    if r_is_altrep(value) {
        unsafe {
            return has_nested_environment(R_altrep_data1(value)) ||
                has_nested_environment(R_altrep_data2(value));
        }
    }

    unsafe {
        // S4 slots are attributes and might be expandable
        // so we need to check if they have reference objects
        if r_is_s4(value) && has_nested_environment(ATTRIB(value)) {
            return true;
        }
    }

    let rtype = r_typeof(value);
    match rtype {
        ENVSXP => true,

        LISTSXP | LANGSXP => unsafe {
            has_nested_environment(CAR(value)) || has_nested_environment(CDR(value))
        },

        VECSXP | EXPRSXP => unsafe {
            let n = Rf_xlength(value);
            let mut has_ref = false;
            for i in 0..n {
                if has_nested_environment(VECTOR_ELT(value, i)) {
                    has_ref = true;
                    break;
                }
            }
            has_ref
        },

        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use libr::Rf_ScalarInteger;
    use libr::Rf_defineVar;

    use super::*;
    use crate::environment::EnvironmentFilter;
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

        let env = Environment::new(test_env, EnvironmentFilter::default());
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
