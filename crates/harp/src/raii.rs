//
// raii.rs
//
// Copyright (C) 2023-2024 by Posit Software, PBC
//
//

pub struct RLocal<T: Copy> {
    old_value: T,
    variable: *mut T,

    // The RAII scopes must be nested to work properly and thus can't be moved
    // or copied. The no copy requirement is fulfilled by having a `Drop` method
    // and this Pin marker prevents moving.
    _pin: std::marker::PhantomPinned,
}

pub struct RLocalOption {
    old_value: crate::RObject,
    option: crate::RSymbol,

    _pin: std::marker::PhantomPinned,
}

pub struct RLocalBoolean {
    _raii: RLocal<libr::Rboolean>,
}

pub struct RLocalInterruptsSuspended {
    _raii: RLocalBoolean,
}

pub struct RLocalInteractive {
    _raii: RLocalBoolean,
}

pub struct RLocalSandbox {
    _interrupts_scope: RLocalInterruptsSuspended,
    _polled_events_scope: crate::sys::polled_events::RLocalPolledEventsSuspended,
}

pub struct RLocalOptionBoolean {
    _raii: RLocalOption,
}

pub struct RLocalShowErrorMessageOption {
    _raii: RLocalOptionBoolean,
}

impl<T> RLocal<T>
where
    T: Copy,
{
    pub fn new(new_value: T, variable: *mut T) -> RLocal<T> {
        unsafe {
            let old_value = libr::get(variable);
            libr::set(variable, new_value);

            Self {
                old_value,
                variable,
                _pin: std::marker::PhantomPinned,
            }
        }
    }
}

impl<T> Drop for RLocal<T>
where
    T: Copy,
{
    fn drop(&mut self) {
        unsafe {
            libr::set(self.variable, self.old_value);
        }
    }
}

impl RLocalOption {
    pub fn new(option: crate::RSymbol, new_value: libr::SEXP) -> RLocalOption {
        let old_value = crate::r_poke_option(option.sexp, new_value);

        Self {
            old_value: old_value.into(),
            option,
            _pin: std::marker::PhantomPinned,
        }
    }
}

impl Drop for RLocalOption {
    fn drop(&mut self) {
        crate::r_poke_option(self.option.sexp, self.old_value.sexp);
    }
}

impl RLocalBoolean {
    pub fn new(value: bool, variable: *mut libr::Rboolean) -> Self {
        let new_value = if value {
            libr::Rboolean_TRUE
        } else {
            libr::Rboolean_FALSE
        };

        Self {
            _raii: RLocal::new(new_value, variable),
        }
    }
}

impl RLocalInterruptsSuspended {
    pub fn new(value: bool) -> Self {
        Self {
            _raii: RLocalBoolean::new(value, unsafe { libr::R_interrupts_suspended }),
        }
    }
}

impl RLocalInteractive {
    pub fn new(value: bool) -> Self {
        Self {
            _raii: RLocalBoolean::new(value, unsafe { libr::R_Interactive }),
        }
    }
}

impl RLocalSandbox {
    pub fn new() -> Self {
        Self {
            _interrupts_scope: RLocalInterruptsSuspended::new(true),
            _polled_events_scope: crate::sys::polled_events::RLocalPolledEventsSuspended::new(true),
        }
    }
}

impl RLocalOptionBoolean {
    pub fn new(option: crate::RSymbol, value: bool) -> Self {
        let new_value: crate::RObject = value.into();

        Self {
            _raii: RLocalOption::new(option, new_value.sexp),
        }
    }
}

impl RLocalShowErrorMessageOption {
    pub fn new(value: bool) -> Self {
        Self {
            _raii: RLocalOptionBoolean::new(
                unsafe { crate::RSymbol::new_unchecked(crate::r_symbol!("show.error.messages")) },
                value,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::raii::RLocalInteractive;
    use crate::raii::RLocalShowErrorMessageOption;
    use crate::test::r_test;

    #[test]
    fn test_local_variable() {
        r_test(|| {
            let get = || unsafe { libr::get(libr::R_Interactive) };
            let old = get();

            {
                let _guard = RLocalInteractive::new(true);
                assert_eq!(get(), libr::Rboolean_TRUE);

                {
                    let _guard = RLocalInteractive::new(false);
                    assert_eq!(get(), libr::Rboolean_FALSE);
                }

                assert_eq!(get(), libr::Rboolean_TRUE);
            }

            assert_eq!(get(), old);
        })
    }

    #[test]
    fn test_local_option() {
        r_test(|| {
            let get = || -> bool {
                unsafe {
                    crate::RObject::view(libr::Rf_GetOption1(crate::r_symbol!(
                        "show.error.messages"
                    )))
                    .try_into()
                    .unwrap()
                }
            };
            let old = get();

            {
                let _guard = RLocalShowErrorMessageOption::new(true);
                assert_eq!(get(), true);

                {
                    let _guard = RLocalShowErrorMessageOption::new(false);
                    assert_eq!(get(), false);
                }

                assert_eq!(get(), true);
            }

            assert_eq!(get(), old);
        })
    }
}
