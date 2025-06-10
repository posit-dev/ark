//
// weak_ref.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use crate::RObject;

/// Weak reference to an R object.
///
/// This is a wrapper around R weak references (see
/// <https://cran.r-project.org/doc/manuals/r-devel/R-exts.html#External-pointers-and-weak-references>).
///
/// The weak reference points to an R object that you can dereference with the
/// `deref()` method, without preventing R from garbage collecting this object.
/// When it gets GC'd by R, or when the weak ref is dropped, the supplied
/// `finalizer` is run. This technique allows you to monitor the existence of R
/// objects in the session.
///
/// Note that just because the weak reference is active does not mean that the
/// object is still reachable. It might be lingering in memory until the next
/// GC.
#[derive(Debug)]
pub struct RWeakRef {
    weak_ref: RObject,
}

impl RWeakRef {
    pub fn new(obj: libr::SEXP, finalizer: impl FnOnce()) -> Self {
        // Convert generic `FnOnce` to a boxed trait object. This must be an
        // explicit `Box<dyn FnOnce()>`, not a `Box<impl FnOnce()>`, which you
        // get if you don't specify the type. The latter is not a proper trait
        // object.
        let finalizer: Box<dyn FnOnce()> = Box::new(finalizer);

        // Since `finalizer` is a trait object we need to double box it before
        // calling `Box::into_raw()`. If we call `into_raw()` directly on the
        // boxed trait object it returns a fat pointer with a vtable, which is
        // not a valid C pointer we can pass across the FFI boundary. So we
        // rebox it and convert the outer box to a pointer. See
        // https://users.rust-lang.org/t/how-to-convert-box-dyn-fn-into-raw-pointer-and-then-call-it/104410.
        let finalizer = Box::new(finalizer);

        // Create a C pointer to the outer box and prevent it from being
        // destructed on drop. The resource will now be managed by R.
        let finalizer = Box::into_raw(finalizer);

        // Wrap that address in an R external pointer.
        let finalizer = RObject::new(unsafe {
            libr::R_MakeExternalPtr(
                finalizer as *mut std::ffi::c_void,
                RObject::null().sexp,
                RObject::null().sexp,
            )
        });

        // This is the C callback that unpacks the external pointer when the
        // weakref is finalized.
        unsafe extern "C-unwind" fn finalize_weak_ref(key: libr::SEXP) {
            let finalizer = libr::R_ExternalPtrAddr(key) as *mut Box<dyn FnOnce()>;

            if finalizer.is_null() {
                log::warn!("Weakref finalizer is unexpectedly NULL");
                return;
            }

            let finalizer = Box::from_raw(finalizer);
            finalizer();
        }

        // Finally, the weakref wraps `obj` and our finalizer.
        let weak_ref = RObject::new(unsafe {
            libr::R_MakeWeakRefC(
                finalizer.sexp, // Protected by weakref
                obj,            // Not protected by weakref
                Some(finalize_weak_ref),
                libr::Rboolean_FALSE,
            )
        });

        Self { weak_ref }
    }

    /// Derefence weakref.
    ///
    /// If the value is `None`, it means the weakref is now stale and the
    /// finalizer has been run.
    pub fn deref(&self) -> Option<RObject> {
        // If finalizer is `NULL` we know for sure the weakref is stale
        let key = unsafe { libr::R_WeakRefKey(self.weak_ref.sexp) };
        if key == RObject::null().sexp {
            return None;
        }

        Some(RObject::new(unsafe {
            libr::R_WeakRefValue(self.weak_ref.sexp)
        }))
    }
}

impl Drop for RWeakRef {
    // It's fine to run this even when weakref is stale
    fn drop(&mut self) {
        unsafe { libr::R_RunWeakRefFinalizer(self.weak_ref.sexp) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::Environment;
    use crate::parse_eval_base;

    #[test]
    fn test_weakref() {
        crate::r_task(|| {
            let env = Environment::new(parse_eval_base("new.env()").unwrap());

            // Destructor runs when weakref is dropped
            let mut has_run = false;
            let weak_ref = RWeakRef::new(env.inner.sexp, || has_run = true);
            drop(weak_ref);
            assert!(has_run);

            // Destructor runs when referee is gc'd
            let mut has_run = false;
            let _weak_ref = RWeakRef::new(env.inner.sexp, || has_run = true);
            drop(env);
            parse_eval_base("gc(full = TRUE)").unwrap();
            assert!(has_run);
        })
    }
}
