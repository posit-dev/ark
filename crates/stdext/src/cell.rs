//
// cell.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! A `RefCell` wrapper that only enforces borrow rules in debug builds.
//!
//! In debug/test builds, `DebugRefCell` delegates to `RefCell` and panics
//! on borrow violations. In release builds, it uses `UnsafeCell` directly
//! with no runtime checking.

#[cfg(not(debug_assertions))]
use std::cell::UnsafeCell;
use std::fmt;
use std::ops::Deref;
use std::ops::DerefMut;

pub struct DebugRefCell<T: ?Sized> {
    #[cfg(debug_assertions)]
    inner: std::cell::RefCell<T>,
    #[cfg(not(debug_assertions))]
    inner: UnsafeCell<T>,
}

// --- Construction & owned access (no guards needed) -------------------------

impl<T> DebugRefCell<T> {
    pub fn new(value: T) -> Self {
        Self {
            #[cfg(debug_assertions)]
            inner: std::cell::RefCell::new(value),
            #[cfg(not(debug_assertions))]
            inner: UnsafeCell::new(value),
        }
    }

    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

impl<T: ?Sized> DebugRefCell<T> {
    /// Exclusive access when you already have `&mut self`.
    /// No runtime check needed in either mode.
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }
}

// --- Shared borrows ---------------------------------------------------------

impl<T: ?Sized> DebugRefCell<T> {
    #[track_caller]
    pub fn borrow(&self) -> DebugRef<'_, T> {
        DebugRef {
            #[cfg(debug_assertions)]
            inner: self.inner.borrow(),
            #[cfg(not(debug_assertions))]
            inner: unsafe { &*self.inner.get() },
        }
    }

    #[track_caller]
    pub fn borrow_mut(&self) -> DebugRefMut<'_, T> {
        DebugRefMut {
            #[cfg(debug_assertions)]
            inner: self.inner.borrow_mut(),
            #[cfg(not(debug_assertions))]
            inner: unsafe { &mut *self.inner.get() },
        }
    }
}

// --- Debug ------------------------------------------------------------------

impl<T: fmt::Debug> fmt::Debug for DebugRefCell<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let borrow = self.borrow();
        f.debug_struct("DebugRefCell")
            .field("value", &*borrow)
            .finish()
    }
}

// --- DebugRef (shared guard) --------------------------------------------------

pub struct DebugRef<'a, T: ?Sized> {
    #[cfg(debug_assertions)]
    inner: std::cell::Ref<'a, T>,
    #[cfg(not(debug_assertions))]
    inner: &'a T,
}

impl<T: ?Sized> Deref for DebugRef<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        #[cfg(debug_assertions)]
        {
            &self.inner
        }
        #[cfg(not(debug_assertions))]
        {
            self.inner
        }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for DebugRef<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

// --- DebugRefMut (exclusive guard) --------------------------------------------

pub struct DebugRefMut<'a, T: ?Sized> {
    #[cfg(debug_assertions)]
    inner: std::cell::RefMut<'a, T>,
    #[cfg(not(debug_assertions))]
    inner: &'a mut T,
}

impl<T: ?Sized> Deref for DebugRefMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        #[cfg(debug_assertions)]
        {
            &self.inner
        }
        #[cfg(not(debug_assertions))]
        {
            self.inner
        }
    }
}

impl<T: ?Sized> DerefMut for DebugRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        #[cfg(debug_assertions)]
        {
            &mut self.inner
        }
        #[cfg(not(debug_assertions))]
        {
            self.inner
        }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for DebugRefMut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_borrow_and_deref() {
        let cell = DebugRefCell::new(42);
        let r = cell.borrow();
        assert_eq!(*r, 42);
    }

    #[test]
    fn test_borrow_mut_and_deref() {
        let cell = DebugRefCell::new(vec![1, 2, 3]);
        cell.borrow_mut().push(4);
        assert_eq!(*cell.borrow(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_multiple_shared_borrows() {
        let cell = DebugRefCell::new(String::from("hello"));
        let r1 = cell.borrow();
        let r2 = cell.borrow();
        assert_eq!(*r1, *r2);
    }

    #[test]
    fn test_into_inner() {
        let cell = DebugRefCell::new(99);
        assert_eq!(cell.into_inner(), 99);
    }

    #[test]
    fn test_get_mut() {
        let mut cell = DebugRefCell::new(10);
        *cell.get_mut() = 20;
        assert_eq!(cell.into_inner(), 20);
    }

    #[test]
    fn test_option_pattern() {
        let cell = DebugRefCell::new(Some(String::from("value")));
        let guard = cell.borrow();
        assert!(guard.is_some());
        assert_eq!(guard.as_ref().unwrap(), "value");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic]
    fn test_conflicting_borrows_panics_in_debug() {
        let cell = DebugRefCell::new(0);
        let _r = cell.borrow();
        let _w = cell.borrow_mut();
    }
}
