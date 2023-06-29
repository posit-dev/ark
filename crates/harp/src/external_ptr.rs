use std::marker::PhantomData;
use std::os::raw::c_void;

use libR_sys::*;

use crate::object::RObject;

pub struct ExternalPointer<'a, T: 'a> {
    pub pointer: RObject,
    phantom: PhantomData<&'a T>,
}

impl<'a, T> ExternalPointer<'a, T> {
    pub unsafe fn new(object: &T) -> Self {
        let pointer = RObject::from(R_MakeExternalPtr(
            object as *const T as *const c_void as *mut c_void,
            R_NilValue,
            R_NilValue,
        ));

        Self {
            pointer,
            phantom: PhantomData,
        }
    }

    pub unsafe fn reference(pointer: SEXP) -> &'static T {
        unsafe { &*(R_ExternalPtrAddr(pointer) as *const c_void as *const T) }
    }
}

impl<'a, T> From<ExternalPointer<'a, T>> for RObject {
    fn from(value: ExternalPointer<'a, T>) -> Self {
        value.pointer
    }
}
