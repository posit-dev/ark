//
// raii.rs
//
// Copyright (C) 2023-2024 by Posit Software, PBC
//
//

// SAFETY: The guards created by `RLocal` structs should never be moved around.
// The guards should always form a nested stack so that restoration to old
// values happens in the expected order. If you move the guards, you might end
// up creating an unexpected order of value restoration.

pub struct RLocal<T: Copy> {
    old_value: T,
    variable: *mut T,
}

pub struct RLocalOption {
    old_value: crate::RObject,
    option: crate::RSymbol,
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
    pub fn new(option: &str, new_value: libr::SEXP) -> RLocalOption {
        let option = crate::RSymbol::new_unchecked(unsafe { crate::r_symbol!(option) });
        let old_value = crate::r_poke_option(option.sexp, new_value);

        Self {
            old_value: old_value.into(),
            option,
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
    pub fn new(option: &str, value: bool) -> Self {
        let new_value: crate::RObject = value.into();

        Self {
            _raii: RLocalOption::new(option, new_value.sexp),
        }
    }
}

impl RLocalShowErrorMessageOption {
    pub fn new(value: bool) -> Self {
        Self {
            _raii: RLocalOptionBoolean::new("show.error.messages", value),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::raii::RLocalInteractive;
    use crate::raii::RLocalShowErrorMessageOption;

    #[test]
    fn test_local_variable() {
        crate::r_task(|| {
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
        crate::r_task(|| {
            let get = || -> bool { harp::get_option("show.error.messages").try_into().unwrap() };
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
