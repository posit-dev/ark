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
