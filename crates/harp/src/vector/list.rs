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

pub struct List {
    inner: RObject,
}

pub struct ListIter {
    index: usize,
    size: usize,
    ptr: *const SEXP,
    _shelter: RObject,
}

impl List {
    pub fn create<T>(data: T) -> crate::Result<Self>
    where
        T: IntoIterator,
        <T as IntoIterator>::IntoIter: ExactSizeIterator,
        <T as IntoIterator>::Item: Into<SEXP>,
    {
        let mut data = data.into_iter();

        let size = data.len();
        let inner = crate::alloc_list(size)?;
        let inner: RObject = inner.into();

        for i in 0..size {
            unsafe {
                let value = data.next().unwrap_unchecked();
                r_list_poke(inner.sexp, i as libr::R_xlen_t, value.into())
            }
        }

        Ok(Self { inner })
    }

    pub fn iter(&self) -> ListIter {
        unsafe { ListIter::new_unchecked(self.inner.sexp) }
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
    use libr::SEXP;

    use crate::r_test;
    use crate::vector::list::List;
    use crate::RObject;

    #[test]
    fn test_list() {
        r_test! {
            let xs = List::create::<[SEXP;0]>([]).unwrap();
            assert!(xs.iter().next().is_none());

            let xs = List::create([RObject::from(1), RObject::from("foo")]).unwrap();
            let mut it = xs.iter();

            assert!(crate::is_identical(it.next().unwrap(), RObject::from(1).sexp));
            assert!(crate::is_identical(it.next().unwrap(), RObject::from("foo").sexp));
            assert!(it.next().is_none());
        }
    }
}
