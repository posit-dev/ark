//
// list.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use libr::SEXP;

use crate::object::list_cbegin;
use crate::object::r_length;
use crate::object::r_list_poke;
use crate::object::RObject;
use crate::r_typeof;

#[derive(Debug)]
pub struct List {
    pub inner: RObject,
    pub sexp: SEXP,
    pub ptr: *const SEXP,
}

pub struct ListIter {
    index: usize,
    size: usize,
    ptr: *const SEXP,
    _shelter: RObject,
}

impl List {
    pub fn iter(&self) -> ListIter {
        unsafe { ListIter::new_unchecked(self.inner.sexp) }
    }
}

impl super::Vector for List {
    type Type = RObject;
    type Item = SEXP;
    const SEXPTYPE: u32 = libr::VECSXP;
    type UnderlyingType = SEXP;
    type CompareType = RObject;

    unsafe fn new_unchecked(object: impl Into<SEXP>) -> Self {
        let object: SEXP = object.into();
        let ptr = crate::list_cbegin(object);

        Self {
            inner: object.into(),
            sexp: object,
            ptr,
        }
    }

    fn data(&self) -> SEXP {
        self.inner.sexp
    }

    // Never missing. We can't treat `NULL` as missing because it would cause
    // getters to return `None` or a `MissingValueError`.
    fn is_na(_x: &Self::UnderlyingType) -> bool {
        false
    }

    fn get_unchecked_elt(&self, index: isize) -> Self::UnderlyingType {
        unsafe { *self.ptr.wrapping_add(index as usize) }
    }

    fn convert_value(x: &Self::UnderlyingType) -> Self::Type {
        (*x).into()
    }

    unsafe fn create<T>(data: T) -> Self
    where
        T: IntoIterator,
        <T as IntoIterator>::IntoIter: ExactSizeIterator,
        <T as IntoIterator>::Item: AsRef<Self::Item>,
    {
        let mut data = data.into_iter();

        let size = data.len();
        let sexp = crate::alloc_list(size).unwrap();
        let inner: RObject = sexp.into();

        for i in 0..size {
            unsafe {
                let value = data.next().unwrap_unchecked();
                let value = value.as_ref();
                r_list_poke(inner.sexp, i as libr::R_xlen_t, *value)
            }
        }

        let ptr = crate::list_cbegin(inner.sexp);

        Self { inner, ptr, sexp }
    }

    fn format_one(&self, _x: Self::Type, _options: Option<&super::FormatOptions>) -> String {
        todo!()
    }
}

impl ListIter {
    pub fn new(x: SEXP) -> crate::Result<Self> {
        match r_typeof(x) {
            libr::VECSXP | libr::EXPRSXP => {},
            _ => {
                let exp = vec![libr::VECSXP, libr::EXPRSXP];
                return Err(crate::Error::UnexpectedType(r_typeof(x), exp));
            },
        };

        Ok(unsafe { Self::new_unchecked(x) })
    }

    /// SAFETY: Assumes `x` is VECSXP or EXPRSXP
    pub unsafe fn new_unchecked(x: SEXP) -> Self {
        let ptr = list_cbegin(x);

        Self {
            index: 0,
            size: r_length(x) as usize,
            ptr,
            _shelter: x.into(),
        }
    }
}

impl std::iter::Iterator for ListIter {
    type Item = SEXP;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.size {
            return None;
        }

        let item = unsafe { *self.ptr.wrapping_add(self.index) };
        self.index = self.index + 1;
        Some(item)
    }
}

#[cfg(test)]
mod test {
    use crate::r_test;
    use crate::vector::list::List;
    use crate::vector::Vector;
    use crate::RObject;

    #[test]
    fn test_list() {
        r_test! {
            let xs = List::create::<[RObject;0]>([]);
            assert!(xs.iter().next().is_none());

            let xs = List::create([RObject::from(1), RObject::from("foo")]);
            let mut it = xs.iter();

            assert!(crate::is_identical(it.next().unwrap(), RObject::from(1).sexp));
            assert!(crate::is_identical(it.next().unwrap(), RObject::from("foo").sexp));
            assert!(it.next().is_none());
        }
    }
}
