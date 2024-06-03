//
// raii.rs
//
// Copyright (C) 2023-2024 by Posit Software, PBC
//
//

pub struct RRaiiScope<T: Copy> {
    old_value: T,
    variable: *mut T,

    // The RAII scopes must be nested to work properly and thus can't be moved
    // or copied. The no copy requirement is fulfilled by having a `Drop` method
    // and this Pin marker prevents moving.
    _pin: std::marker::PhantomPinned,
}

pub struct RRaiiBooleanScope {
    _raii: RRaiiScope<libr::Rboolean>,
}

pub struct RRaiiOptionScope<T: Copy> {
    _raii: RRaiiScope<Option<T>>,
}

pub struct RInterruptsSuspendedScope {
    _raii: RRaiiBooleanScope,
}

pub struct RInteractiveScope {
    _raii: RRaiiBooleanScope,
}

pub struct RSandboxScope {
    _interrupts_scope: RInterruptsSuspendedScope,
    _polled_events_scope: crate::sys::polled_events::RPolledEventsSuspendedScope,
}

impl<T> RRaiiScope<T>
where
    T: Copy,
{
    pub fn new(new_value: T, variable: *mut T) -> RRaiiScope<T> {
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

impl<T> Drop for RRaiiScope<T>
where
    T: Copy,
{
    fn drop(&mut self) {
        unsafe {
            libr::set(self.variable, self.old_value);
        }
    }
}

impl RRaiiBooleanScope {
    pub fn new(value: bool, variable: *mut libr::Rboolean) -> Self {
        let new_value = if value == false {
            libr::Rboolean_FALSE
        } else {
            libr::Rboolean_TRUE
        };

        Self {
            _raii: RRaiiScope::new(new_value, variable),
        }
    }
}

impl RInterruptsSuspendedScope {
    pub fn new(value: bool) -> Self {
        Self {
            _raii: RRaiiBooleanScope::new(value, unsafe { libr::R_interrupts_suspended }),
        }
    }
}

impl RInteractiveScope {
    pub fn new(value: bool) -> Self {
        Self {
            _raii: RRaiiBooleanScope::new(value, unsafe { libr::R_Interactive }),
        }
    }
}

impl RSandboxScope {
    pub fn new() -> Self {
        Self {
            _interrupts_scope: RInterruptsSuspendedScope::new(true),
            _polled_events_scope: crate::sys::polled_events::RPolledEventsSuspendedScope::new(true),
        }
    }
}
