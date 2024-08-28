//
// object.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::convert::TryFrom;
use std::i32;
use std::ops::Deref;
use std::os::raw::c_char;
use std::os::raw::c_int;
use std::sync::Once;

use libc::c_double;
use libr::*;

use crate::error::Error;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::protect::RProtect;
use crate::r_symbol;
use crate::utils::r_assert_capacity;
use crate::utils::r_assert_length;
use crate::utils::r_assert_type;
use crate::utils::r_chr_get_owned_utf8;
use crate::utils::r_is_altrep;
use crate::utils::r_is_null;
use crate::utils::r_is_object;
use crate::utils::r_is_s4;
use crate::utils::r_str_to_owned_utf8;
use crate::utils::r_typeof;

// Objects are protected using a doubly-linked list,
// allowing for quick insertion and removal of objects.
static PRECIOUS_LIST_ONCE: Once = Once::new();
static mut PRECIOUS_LIST: Option<SEXP> = None;

unsafe fn protect(object: SEXP) -> SEXP {
    // Nothing to do
    if r_is_null(object) {
        return R_NilValue;
    }

    // Protect the incoming object, just in case.
    Rf_protect(object);

    // Initialize the precious list.
    PRECIOUS_LIST_ONCE.call_once(|| {
        let precious_list = Rf_cons(R_NilValue, Rf_cons(R_NilValue, R_NilValue));
        R_PreserveObject(precious_list);
        PRECIOUS_LIST = Some(precious_list);
    });

    let precious_list = PRECIOUS_LIST.unwrap_unchecked();

    // Get references to the head, tail of the current precious list.
    let head = precious_list;
    let tail = CDR(precious_list);

    // The new cell will be inserted between the existing head and tail,
    // so create a new cell referencing the head and tail of the list.
    let cell = Rf_protect(Rf_cons(head, tail));

    // Set the TAG on the cell so the object is protected.
    SET_TAG(cell, object);

    // Point the CDR of the current head to the newly-created cell.
    SETCDR(head, cell);

    // Point the CAR of the current tail to the newly-created cell.
    SETCAR(tail, cell);

    // Clean up the protect stack and return.
    Rf_unprotect(2);

    // Uncomment if debugging protection issues
    // trace!("Protecting cell:   {:?}", cell);
    return cell;
}

unsafe fn unprotect(cell: SEXP) {
    if r_is_null(cell) {
        return;
    }

    // Uncomment if debugging protection issues
    // trace!("Unprotecting cell: {:?}", cell);

    // We need to remove the cell from the precious list.
    // The CAR of the cell points to the previous cell in the precious list.
    // The CDR of the cell points to the next cell in the precious list.
    let head = CAR(cell);
    let tail = CDR(cell);

    // Point the head back at the tail.
    SETCDR(head, tail);

    // Point the tail back at the head.
    SETCAR(tail, head);

    // There should now be no references to the cell above, allowing it
    // (and the object it contains) to be cleaned up.
    SET_TAG(cell, R_NilValue);
}

#[derive(Debug)]
pub struct RObject {
    pub sexp: SEXP,
    pub cell: SEXP,
}

pub trait RObjectExt<T> {
    unsafe fn elt(&self, index: T) -> crate::error::Result<RObject>;
}

impl PartialEq for RObject {
    fn eq(&self, other: &Self) -> bool {
        self.sexp == other.sexp
    }
}
impl Eq for RObject {}

impl<T: Into<RObject>> RObjectExt<T> for RObject {
    unsafe fn elt(&self, index: T) -> crate::error::Result<RObject> {
        let index: RObject = index.into();
        RFunction::new("base", "[[")
            .add(self.sexp)
            .add(index)
            .call()
    }
}

// TODO: borrow implementation from lobstr instead
//       of calling object.size()
fn r_size(x: SEXP) -> usize {
    if r_is_null(x) {
        return 0;
    }
    if r_is_altrep(x) {
        return unsafe { r_size(R_altrep_data1(x)) + r_size(R_altrep_data2(x)) };
    }
    let size = RFunction::new("utils", "object.size").add(x).call();

    match size {
        Err(_) => 0,
        Ok(size) => {
            let value = unsafe { REAL_ELT(*size, 0) };
            value as usize
        },
    }
}

pub fn r_length(x: SEXP) -> isize {
    unsafe { Rf_xlength(x) }
}

pub fn r_dim(x: SEXP) -> SEXP {
    unsafe { Rf_getAttrib(x, R_DimSymbol) }
}

pub fn r_lgl_get(x: SEXP, i: isize) -> i32 {
    unsafe { LOGICAL_ELT(x, i) }
}
pub fn r_int_get(x: SEXP, i: isize) -> i32 {
    unsafe { INTEGER_ELT(x, i) }
}
pub fn r_dbl_get(x: SEXP, i: isize) -> f64 {
    unsafe { REAL_ELT(x, i) }
}
pub fn r_cpl_get(x: SEXP, i: isize) -> Rcomplex {
    unsafe { COMPLEX_ELT(x, i) }
}
pub fn r_chr_get(x: SEXP, i: isize) -> SEXP {
    unsafe { STRING_ELT(x, i) }
}

// TODO: Once we have a Rust list type, move this back to unsafe.
// Should be unsafe because the type and bounds are not checked and
// will result in a crash if used incorrectly.
pub fn list_get(x: SEXP, i: isize) -> SEXP {
    unsafe { VECTOR_ELT(x, i) }
}

pub fn list_poke(x: SEXP, i: isize, value: SEXP) {
    unsafe { SET_VECTOR_ELT(x, i, value) };
}

pub fn r_lgl_na() -> i32 {
    unsafe { R_NaInt }
}
pub fn r_int_na() -> i32 {
    unsafe { R_NaInt }
}
pub fn r_dbl_na() -> f64 {
    unsafe { R_NaReal }
}
pub fn r_str_na() -> SEXP {
    unsafe { R_NaString }
}

pub fn r_str_blank() -> SEXP {
    unsafe { R_BlankString }
}

pub fn r_dbl_nan() -> f64 {
    unsafe { R_NaN }
}
pub fn r_dbl_positive_infinity() -> f64 {
    unsafe { R_PosInf }
}
pub fn r_dbl_negative_infinity() -> f64 {
    unsafe { R_NegInf }
}

pub fn r_dbl_is_na(x: f64) -> bool {
    unsafe { R_IsNA(x) != 0 }
}
pub fn r_dbl_is_nan(x: f64) -> bool {
    unsafe { R_IsNaN(x) != 0 }
}
/// Returns `true` if `x` is not `NA`, `NaN`, `Inf`, or `-Inf`
pub fn r_dbl_is_finite(x: f64) -> bool {
    unsafe { R_finite(x) != 0 }
}

pub fn r_lgl_poke(x: SEXP, i: R_xlen_t, value: i32) {
    unsafe { SET_LOGICAL_ELT(x, i, value) }
}
pub fn r_int_poke(x: SEXP, i: R_xlen_t, value: i32) {
    unsafe { SET_INTEGER_ELT(x, i, value) }
}
pub fn r_dbl_poke(x: SEXP, i: R_xlen_t, value: f64) {
    unsafe { SET_REAL_ELT(x, i, value) }
}
pub fn r_chr_poke(x: SEXP, i: R_xlen_t, value: SEXP) {
    unsafe { SET_STRING_ELT(x, i, value) }
}
pub fn r_list_poke(x: SEXP, i: R_xlen_t, value: SEXP) {
    unsafe {
        SET_VECTOR_ELT(x, i, value);
    }
}

pub fn r_lgl_begin(x: SEXP) -> *mut i32 {
    unsafe { LOGICAL(x) }
}
pub fn r_int_begin(x: SEXP) -> *mut i32 {
    unsafe { INTEGER(x) }
}
pub fn r_dbl_begin(x: SEXP) -> *mut f64 {
    unsafe { REAL(x) }
}

// TODO: Make these wrappers robust to allocation failures
// https://github.com/posit-dev/positron/issues/2600
pub fn r_alloc_logical(size: R_xlen_t) -> SEXP {
    unsafe { Rf_allocVector(LGLSXP, size) }
}
pub fn r_alloc_integer(size: R_xlen_t) -> SEXP {
    unsafe { Rf_allocVector(INTSXP, size) }
}
pub fn r_alloc_double(size: R_xlen_t) -> SEXP {
    unsafe { Rf_allocVector(REALSXP, size) }
}
pub fn r_alloc_complex(size: R_xlen_t) -> SEXP {
    unsafe { Rf_allocVector(CPLXSXP, size) }
}
pub fn r_alloc_character(size: R_xlen_t) -> SEXP {
    unsafe { Rf_allocVector(STRSXP, size) }
}
pub fn r_alloc_list(size: R_xlen_t) -> SEXP {
    unsafe { Rf_allocVector(VECSXP, size) }
}

pub fn r_node_car(x: SEXP) -> SEXP {
    unsafe { CAR(x) }
}
pub fn r_node_tag(x: SEXP) -> SEXP {
    unsafe { TAG(x) }
}
pub fn r_node_cdr(x: SEXP) -> SEXP {
    unsafe { CDR(x) }
}

impl RObject {
    pub unsafe fn new(data: SEXP) -> Self {
        RObject {
            sexp: data,
            cell: protect(data),
        }
    }

    pub fn view(data: SEXP) -> Self {
        RObject {
            sexp: data,
            cell: unsafe { R_NilValue },
        }
    }

    pub fn null() -> Self {
        RObject {
            sexp: unsafe { R_NilValue },
            cell: unsafe { R_NilValue },
        }
    }

    // A helper function that makes '.try_into()' more ergonomic to use.
    pub unsafe fn to<U: TryFrom<RObject, Error = crate::error::Error>>(self) -> Result<U, Error> {
        TryInto::<U>::try_into(self)
    }

    pub fn is_s4(&self) -> bool {
        r_is_s4(self.sexp)
    }

    pub fn is_altrep(&self) -> bool {
        r_is_altrep(self.sexp)
    }

    pub fn is_object(&self) -> bool {
        r_is_object(self.sexp)
    }

    pub fn size(&self) -> usize {
        r_size(self.sexp)
    }

    pub fn length(&self) -> isize {
        r_length(self.sexp)
    }

    pub fn kind(&self) -> u32 {
        r_typeof(self.sexp)
    }

    /// String accessor; get a string value from a vector of strings.
    ///
    /// - `idx` - The index of the string to return.
    ///
    /// Returns the string at the given index, or None if the string is NA.
    pub fn get_string(&self, idx: isize) -> crate::error::Result<Option<String>> {
        unsafe {
            r_assert_type(self.sexp, &[STRSXP])?;
            r_assert_capacity(self.sexp, idx as usize)?;
            let charsexp = STRING_ELT(self.sexp, idx);
            if charsexp == R_NaString {
                Ok(None)
            } else {
                Ok(Some(RObject::view(charsexp).try_into()?))
            }
        }
    }

    /// Integer accessor; get an integer value from a vector of integers.
    ///
    /// - `idx` - The index of the integer to return.
    ///
    /// Returns the intger at the given index, or None if the integer is NA.
    pub fn get_i32(&self, idx: isize) -> crate::error::Result<Option<i32>> {
        unsafe {
            r_assert_type(self.sexp, &[INTSXP])?;
            r_assert_capacity(self.sexp, idx as usize)?;
            let intval = INTEGER_ELT(self.sexp, idx);
            if intval == R_NaInt {
                Ok(None)
            } else {
                Ok(Some(intval))
            }
        }
    }

    /// Real-value accessor; get an real (floating point) value from a vector.
    ///
    /// - `idx` - The index of the value to return.
    ///
    /// Returns the real value at the given index, or None if the value is NA.
    pub fn get_f64(&self, idx: isize) -> crate::error::Result<Option<f64>> {
        unsafe {
            r_assert_type(self.sexp, &[REALSXP])?;
            r_assert_capacity(self.sexp, idx as usize)?;
            let f64val = REAL_ELT(self.sexp, idx);
            if f64val == R_NaReal {
                Ok(None)
            } else {
                Ok(Some(f64val))
            }
        }
    }

    /// Logical-value accessor; get a logical (boolean) value from a vector.
    ///
    /// - `idx` - The index of the value to return.
    ///
    /// Returns the logical value at the given index, or None if the value is
    /// NA.
    pub fn get_bool(&self, idx: isize) -> crate::error::Result<Option<bool>> {
        unsafe {
            r_assert_type(self.sexp, &[LGLSXP])?;
            r_assert_capacity(self.sexp, idx as usize)?;
            let boolval = LOGICAL_ELT(self.sexp, idx);
            if boolval == R_NaInt {
                Ok(None)
            } else {
                Ok(Some(boolval != 0))
            }
        }
    }

    /// Vector (list) accessor; get a vector value from a list as another
    /// RObject.
    ///
    /// - `idx` - The index of the value to return.
    ///
    /// Returns an RObject representing the value at the given index.
    pub fn vector_elt(&self, idx: isize) -> crate::error::Result<RObject> {
        unsafe {
            r_assert_type(self.sexp, &[VECSXP])?;
            r_assert_capacity(self.sexp, idx as usize)?;
            Ok(RObject::new(VECTOR_ELT(self.sexp, idx)))
        }
    }

    /// Gets a vector containing names for the object's values (from the `names`
    /// attribute). Returns `None` if the object's value(s) don't have names.
    pub fn names(&self) -> Option<Vec<Option<String>>> {
        let names = unsafe { Rf_getAttrib(self.sexp, R_NamesSymbol) };
        let names = RObject::view(names);
        match names.kind() {
            STRSXP => Vec::<Option<String>>::try_from(names).ok(),
            _ => None,
        }
    }

    /// Gets a named attribute from the object. Returns `None` if the attribute
    /// doesn't exist.
    pub fn attr(&self, name: &str) -> Option<RObject> {
        let val = unsafe { Rf_getAttrib(self.sexp, r_symbol!(name)) };
        if r_is_null(val) {
            None
        } else {
            Some(unsafe { RObject::new(val) })
        }
    }

    pub fn set_attr(&self, name: &str, value: SEXP) {
        unsafe {
            Rf_protect(value);
            Rf_setAttrib(self.sexp, r_symbol!(name), value);
            Rf_unprotect(1);
        }
    }

    pub fn duplicate(&self) -> RObject {
        unsafe { RObject::new(libr::Rf_duplicate(self.sexp)) }
    }

    pub fn shallow_duplicate(&self) -> RObject {
        unsafe { RObject::new(libr::Rf_shallow_duplicate(self.sexp)) }
    }
}

impl Clone for RObject {
    fn clone(&self) -> Self {
        let sexp = self.sexp;
        let cell = if r_is_null(self.cell) {
            self.cell
        } else {
            unsafe { protect(sexp) }
        };
        Self { sexp, cell }
    }
}

impl Drop for RObject {
    fn drop(&mut self) {
        unsafe {
            unprotect(self.cell);
        }
    }
}

// SAFETY: Neither `Sync` nor `Send` are safe to implement for `RObject`. Even
// with `Sync`, you can call methods from `&RObject` while on different threads,
// which could call the R API. Instead, use `RThreadSafe<RObject>` to send
// across threads.
// unsafe impl Sync for RObject {}
// unsafe impl Send for RObject {}

impl Deref for RObject {
    type Target = SEXP;
    fn deref(&self) -> &Self::Target {
        &self.sexp
    }
}

/// Convert other object types into RObjects.
impl From<SEXP> for RObject {
    fn from(value: SEXP) -> Self {
        unsafe { RObject::new(value) }
    }
}

impl From<()> for RObject {
    fn from(_value: ()) -> Self {
        unsafe { RObject::from(R_NilValue) }
    }
}

impl From<bool> for RObject {
    fn from(value: bool) -> Self {
        unsafe {
            let value = Rf_ScalarLogical(value as c_int);
            return RObject::new(value);
        }
    }
}

impl From<i32> for RObject {
    fn from(value: i32) -> Self {
        unsafe {
            let value = Rf_ScalarInteger(value as c_int);
            return RObject::new(value);
        }
    }
}

impl TryFrom<i64> for RObject {
    type Error = crate::error::Error;
    fn try_from(value: i64) -> Result<Self, Error> {
        unsafe {
            // Ensure the value is within the range of an i32.
            if value < i32::MIN as i64 || value > i32::MAX as i64 {
                return Err(Error::ValueOutOfRange {
                    value,
                    min: i32::MIN as i64,
                    max: i32::MAX as i64,
                });
            }
            let value = Rf_ScalarInteger(value as c_int);
            return Ok(RObject::new(value));
        }
    }
}

impl From<f64> for RObject {
    fn from(value: f64) -> Self {
        unsafe {
            let value = Rf_ScalarReal(value);
            return RObject::new(value);
        }
    }
}

impl From<&str> for RObject {
    fn from(value: &str) -> Self {
        unsafe {
            let vector = Rf_protect(Rf_allocVector(STRSXP, 1));
            let element = Rf_mkCharLenCE(
                value.as_ptr() as *mut c_char,
                value.len() as i32,
                cetype_t_CE_UTF8,
            );
            SET_STRING_ELT(vector, 0, element);
            Rf_unprotect(1);
            return RObject::new(vector);
        }
    }
}

impl From<String> for RObject {
    fn from(value: String) -> Self {
        value.as_str().into()
    }
}

impl From<Vec<String>> for RObject {
    fn from(values: Vec<String>) -> Self {
        unsafe {
            let vector = RObject::from(Rf_allocVector(STRSXP, values.len() as isize));
            for idx in 0..values.len() {
                let value_str = Rf_mkCharLenCE(
                    values[idx].as_ptr() as *mut c_char,
                    values[idx].len() as i32,
                    cetype_t_CE_UTF8,
                );
                SET_STRING_ELT(vector.sexp, idx as isize, value_str);
            }
            return vector;
        }
    }
}

impl From<&Vec<i64>> for RObject {
    fn from(values: &Vec<i64>) -> Self {
        unsafe {
            let vector = RObject::from(Rf_allocVector(INTSXP, values.len() as isize));
            for idx in 0..values.len() {
                SET_INTEGER_ELT(vector.sexp, idx as isize, values[idx] as c_int);
            }
            return vector;
        }
    }
}

impl From<&Vec<f64>> for RObject {
    fn from(values: &Vec<f64>) -> Self {
        unsafe {
            let vector = RObject::from(Rf_allocVector(REALSXP, values.len() as isize));
            for idx in 0..values.len() {
                SET_REAL_ELT(vector.sexp, idx as isize, values[idx] as c_double);
            }
            return vector;
        }
    }
}

// Convert a String -> String HashMap into named character vector.
impl From<HashMap<String, String>> for RObject {
    fn from(value: HashMap<String, String>) -> Self {
        unsafe {
            // Allocate the vector of values
            let values = Rf_protect(Rf_allocVector(STRSXP, value.len() as isize));

            // Allocate the vector of names; this will be protected by attaching
            // it to the values vector as an attribute
            let names = Rf_allocVector(STRSXP, value.len() as isize);
            Rf_setAttrib(values, R_NamesSymbol, names);

            // Convert the hashmap to a sorted vector of tuples; we do this so that the
            // order of the values and names is deterministic
            let mut sorted: Vec<_> = value.into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));

            // Loop over the values and names, setting them in the vectors
            for (idx, (key, value)) in sorted.iter().enumerate() {
                let value_str = Rf_mkCharLenCE(
                    value.as_ptr() as *mut c_char,
                    value.len() as i32,
                    cetype_t_CE_UTF8,
                );
                SET_STRING_ELT(values, idx as isize, value_str);
                let key_str = Rf_mkCharLenCE(
                    key.as_ptr() as *mut c_char,
                    key.len() as i32,
                    cetype_t_CE_UTF8,
                );
                SET_STRING_ELT(names, idx as isize, key_str);
            }

            // Clean up the protect stack and return the RObject from the values
            // vector
            Rf_unprotect(1);
            RObject::new(values)
        }
    }
}

/// Convert RObject into other types.

impl From<RObject> for SEXP {
    fn from(object: RObject) -> Self {
        object.sexp
    }
}

impl TryFrom<RObject> for Option<bool> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_type(*value, &[LGLSXP])?;
            r_assert_length(*value, 1)?;
            let x = *LOGICAL(*value);
            if x == R_NaInt {
                return Ok(None);
            }
            Ok(Some(x != 0))
        }
    }
}

impl TryFrom<RObject> for Option<String> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            let charsexp = match r_typeof(*value) {
                CHARSXP => *value,
                STRSXP => {
                    r_assert_length(*value, 1)?;
                    STRING_ELT(*value, 0)
                },
                SYMSXP => PRINTNAME(*value),
                _ => {
                    return Err(Error::UnexpectedType(r_typeof(*value), vec![
                        CHARSXP, STRSXP, SYMSXP,
                    ]))
                },
            };

            if charsexp == R_NaString {
                return Ok(None);
            }

            let translated = r_str_to_owned_utf8(charsexp)?;
            Ok(Some(translated))
        }
    }
}

impl TryFrom<RObject> for Option<u16> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_length(*value, 1)?;
            match r_typeof(*value) {
                INTSXP => {
                    let x = INTEGER_ELT(*value, 0);
                    if x == R_NaInt {
                        Ok(None)
                    } else if x < u16::MIN as i32 || x > u16::MAX as i32 {
                        Err(Error::ValueOutOfRange {
                            value: x as i64,
                            min: u16::MIN as i64,
                            max: u16::MAX as i64,
                        })
                    } else {
                        Ok(Some(x as u16))
                    }
                },
                _ => Err(Error::UnexpectedType(r_typeof(*value), vec![INTSXP])),
            }
        }
    }
}

impl TryFrom<RObject> for Option<i32> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_length(*value, 1)?;
            match r_typeof(*value) {
                INTSXP => {
                    let x = INTEGER_ELT(*value, 0);
                    if x == R_NaInt {
                        Ok(None)
                    } else {
                        Ok(Some(x))
                    }
                },
                _ => Err(Error::UnexpectedType(r_typeof(*value), vec![INTSXP])),
            }
        }
    }
}

impl TryFrom<RObject> for Option<f64> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_length(*value, 1)?;
            match r_typeof(*value) {
                INTSXP => {
                    let x = INTEGER_ELT(*value, 0);
                    if x == R_NaInt {
                        Ok(None)
                    } else {
                        Ok(Some(x as f64))
                    }
                },
                REALSXP => {
                    let x = REAL_ELT(*value, 0);
                    if R_IsNA(x) != 0 {
                        Ok(None)
                    } else {
                        Ok(Some(x))
                    }
                },
                _ => Err(Error::UnexpectedType(r_typeof(*value), vec![REALSXP])),
            }
        }
    }
}

impl TryFrom<RObject> for String {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        match Option::<String>::try_from(value)? {
            Some(x) => Ok(x),
            None => Err(Error::MissingValueError),
        }
    }
}

impl TryFrom<RObject> for bool {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        match Option::<bool>::try_from(value)? {
            Some(x) => Ok(x),
            None => Err(Error::MissingValueError),
        }
    }
}

impl TryFrom<RObject> for u16 {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        match Option::<u16>::try_from(value)? {
            Some(x) => Ok(x),
            None => Err(Error::MissingValueError),
        }
    }
}

impl TryFrom<RObject> for i32 {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        match Option::<i32>::try_from(value)? {
            Some(x) => Ok(x),
            None => Err(Error::MissingValueError),
        }
    }
}

impl TryFrom<RObject> for f64 {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        match Option::<f64>::try_from(value)? {
            Some(x) => Ok(x),
            None => Err(Error::MissingValueError),
        }
    }
}

impl TryFrom<RObject> for Vec<String> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_type(*value, &[STRSXP, NILSXP])?;

            let mut result: Vec<String> = Vec::new();
            let n = Rf_xlength(*value);
            for i in 0..n {
                let res = r_chr_get_owned_utf8(*value, i)?;
                result.push(res);
            }

            return Ok(result);
        }
    }
}

impl TryFrom<RObject> for Vec<Option<String>> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_type(*value, &[STRSXP, NILSXP])?;

            let n = Rf_xlength(*value);
            let mut result: Vec<Option<String>> = Vec::with_capacity(n as usize);
            for i in 0..n {
                result.push(value.get_string(i as isize)?);
            }
            return Ok(result);
        }
    }
}

impl TryFrom<RObject> for Vec<i32> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_type(value.sexp, &[INTSXP, NILSXP])?;
            if r_is_null(value.sexp) {
                return Ok(Vec::new());
            }

            let n = Rf_xlength(value.sexp);
            let mut result: Vec<i32> = Vec::with_capacity(n as usize);
            for i in 0..n {
                let res = INTEGER_ELT(value.sexp, i);
                if res == R_NaInt {
                    return Err(Error::MissingValueError);
                }
                result.push(res);
            }

            return Ok(result);
        }
    }
}

impl TryFrom<RObject> for Vec<RObject> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_type(value.sexp, &[VECSXP])?;

            let n = Rf_xlength(value.sexp);
            let mut result: Vec<RObject> = Vec::with_capacity(n as usize);
            for i in 0..n {
                let res = value.vector_elt(i)?;
                result.push(res);
            }

            return Ok(result);
        }
    }
}

impl TryFrom<Vec<RObject>> for RObject {
    type Error = crate::error::Error;
    fn try_from(value: Vec<RObject>) -> Result<Self, Self::Error> {
        unsafe {
            let n = value.len();

            // Create the list object.
            let out_raw = Rf_allocVector(VECSXP, n as R_xlen_t);
            let out = RObject::new(out_raw);

            // Copy the values into the list.
            for i in 0..n {
                r_list_poke(out.sexp, i as isize, value[i].sexp)
            }

            return Ok(out);
        }
    }
}

impl TryFrom<&Vec<bool>> for RObject {
    type Error = crate::error::Error;

    // NOTE: Can't currently return `Err`, but will once we add R memory allocators that
    // can fail
    fn try_from(value: &Vec<bool>) -> Result<Self, Self::Error> {
        let n = value.len();

        let out = RObject::from(r_alloc_logical(n as R_xlen_t));
        let v_out = r_lgl_begin(out.sexp);

        for i in 0..n {
            unsafe {
                *v_out.add(i) = value[i] as i32;
            }
        }

        return Ok(out);
    }
}

impl TryFrom<&Vec<i32>> for RObject {
    type Error = crate::error::Error;
    fn try_from(value: &Vec<i32>) -> Result<Self, Self::Error> {
        unsafe {
            let n = value.len();

            let out_raw = Rf_allocVector(INTSXP, n as R_xlen_t);
            let out = RObject::new(out_raw);
            let v_out = DATAPTR(out_raw) as *mut i32;

            for i in 0..n {
                let x = value[i];
                if x == R_NaInt {
                    return Err(crate::Error::MissingValueError);
                }
                *(v_out.offset(i as isize)) = x;
            }

            return Ok(out);
        }
    }
}

// Converts an R named character vector to a HashMap<String, String>
// Note: Duplicated names are silently ignored, and only the first occurence is kept.
impl TryFrom<RObject> for HashMap<String, String> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_type(*value, &[STRSXP, VECSXP])?;

            let mut protect = RProtect::new();
            let names = protect.add(Rf_getAttrib(*value, R_NamesSymbol));
            r_assert_type(names, &[STRSXP])?;

            let value = protect.add(Rf_coerceVector(*value, STRSXP));

            let n = Rf_xlength(names);
            let mut map = HashMap::<String, String>::with_capacity(n as usize);

            for i in (0..Rf_xlength(names)).rev() {
                // Translate the name and value into Rust strings.
                let lhs = r_chr_get_owned_utf8(names, i)?;
                let rhs = r_chr_get_owned_utf8(value, i)?;

                map.insert(lhs, rhs);
            }

            Ok(map)
        }
    }
}

// Converts an R named integer vector to a HashMap<String, i32>
// Note: Duplicated names are silently ignored, and only the first occurence is kept.
impl TryFrom<RObject> for HashMap<String, i32> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            r_assert_type(*value, &[INTSXP, VECSXP])?;

            let mut protect = RProtect::new();
            let names = protect.add(Rf_getAttrib(*value, R_NamesSymbol));
            r_assert_type(names, &[STRSXP])?;

            let value = protect.add(Rf_coerceVector(*value, INTSXP));

            let n = Rf_xlength(names);
            let mut map = HashMap::<String, i32>::with_capacity(n as usize);

            for i in (0..Rf_xlength(names)).rev() {
                // Translate the name and value into Rust strings.
                let name = r_chr_get_owned_utf8(names, i)?;
                let val = r_int_get(value, i);

                map.insert(name, val);
            }

            Ok(map)
        }
    }
}

// Converts a named R object into a HashMap<String, RObject> whose names are used as keys.
// Duplicated names are silently ignored, and only the first occurence is kept.
impl TryFrom<RObject> for HashMap<String, RObject> {
    type Error = crate::error::Error;
    fn try_from(value: RObject) -> Result<Self, Self::Error> {
        unsafe {
            let mut protect = RProtect::new();
            let names = protect.add(Rf_getAttrib(*value, R_NamesSymbol));
            r_assert_type(names, &[STRSXP])?;

            let n = Rf_xlength(names);
            let mut map = HashMap::<String, RObject>::with_capacity(n as usize);

            // iterate in the reverse order to keep the first occurence of a name
            for i in (0..n).rev() {
                let name = r_chr_get_owned_utf8(names, i)?;
                let value: RObject = RObject::new(VECTOR_ELT(*value, i));
                map.insert(name, value);
            }

            Ok(map)
        }
    }
}

pub fn r_null_or_try_into<T>(x: RObject) -> harp::Result<Option<T>>
where
    RObject: TryInto<T, Error = harp::Error>,
{
    if x.sexp == crate::r_null() {
        Ok(None)
    } else {
        Ok(Some(x.try_into()?))
    }
}

#[cfg(test)]
mod tests {
    use libr::SET_STRING_ELT;

    use super::*;
    use crate::assert_match;
    use crate::environment::R_ENVS;
    use crate::eval::r_parse_eval0;
    use crate::r_char;
    use crate::r_test;

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_bool() {
        r_test! {
            assert_match!(
                Option::<bool>::try_from(RObject::from(Rf_ScalarLogical(R_NaInt))),
                Ok(None) => {}
            );
            assert_eq!(
                Option::<bool>::try_from(RObject::from(true)).unwrap(),
                Some(true)
            );
            assert_eq!(
                Option::<bool>::try_from(RObject::from(false)).unwrap(),
                Some(false)
            );
            assert_match!(
                bool::try_from(RObject::from(Rf_ScalarLogical(R_NaInt))),
                Err(Error::MissingValueError) => {}
            );
            assert!(bool::try_from(RObject::from(true)).unwrap());
            assert!(!bool::try_from(RObject::from(false)).unwrap());
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_u16() {
        r_test! {
            // -------------------------------------------------------------------------------------
            // Option::<u16>::try_from tests.
            // -------------------------------------------------------------------------------------

            // Test that R_NaInt is None.
            assert_match!(
                Option::<u16>::try_from(RObject::from(R_NaInt)),
                Ok(None) => {}
            );

            // Test that below range is as error.
            {
                let test_value = (u16::MIN as i32) - 1;
                assert_match!(
                    Option::<u16>::try_from(RObject::from(test_value)),
                    Err(Error::ValueOutOfRange { value, min, max }) => {
                        assert_eq!(value, test_value as i64);
                        assert_eq!(min, u16::MIN as i64);
                        assert_eq!(max, u16::MAX as i64);
                    }
                );
            }

            // Test that above range is None.
            {
                let test_value = (u16::MAX as i32) + 1;
                assert_match!(
                    Option::<u16>::try_from(RObject::from(test_value)),
                    Err(Error::ValueOutOfRange { value, min, max }) => {
                        assert_eq!(value, test_value as i64);
                        assert_eq!(min, u16::MIN as i64);
                        assert_eq!(max, u16::MAX as i64);
                    }
                );
            }

            // Test that minimum value is OK.
            assert_match!(
                Option::<u16>::try_from(RObject::from(u16::MIN as i32)),
                Ok(Some(x)) => {
                    assert_eq!(x, u16::MIN)
                }
            );

            // Test that maximum value is OK.
            assert_match!(
                Option::<u16>::try_from(RObject::from(u16::MAX as i32)),
                Ok(Some(x)) => {
                    assert_eq!(x, u16::MAX)
                }
            );

            // Test that some u16 value is OK.
            assert_match!(
                Option::<u16>::try_from(RObject::from(42)),
                Ok(Some(x)) => {
                    assert_eq!(x, 42)
                }
            );

            // Test that R_NaReal is an error.
            assert_match!(
                Option::<u16>::try_from(RObject::from(R_NaReal)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );

            // Test that some f64 is an error.
            assert_match!(
                Option::<u16>::try_from(RObject::from(42.0)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );

            // -------------------------------------------------------------------------------------
            // u16::try_from tests.
            // -------------------------------------------------------------------------------------

            // Test that R_NaInt is an error.
            assert_match!(
                u16::try_from(RObject::from(R_NaInt)),
                Err(Error::MissingValueError) => {}
            );

            // Test that below range is an error.
            {
                let test_value = (u16::MIN as i32) - 1;
                assert_match!(
                    u16::try_from(RObject::from((u16::MIN as i32) - 1)),
                    Err(Error::ValueOutOfRange { value, min, max }) => {
                        assert_eq!(value, test_value as i64);
                        assert_eq!(min, u16::MIN as i64);
                        assert_eq!(max, u16::MAX as i64);
                    }
                );
            }

            // Test that above range is an error.
            {
                let test_value = (u16::MAX as i32) + 1;
                assert_match!(
                    u16::try_from(RObject::from((u16::MAX as i32) + 1)),
                    Err(Error::ValueOutOfRange { value, min, max }) => {
                        assert_eq!(value, test_value as i64);
                        assert_eq!(min, u16::MIN as i64);
                        assert_eq!(max, u16::MAX as i64);
                    }
                );
            }

            // Test that minimum value is OK.
            assert_match!(
                u16::try_from(RObject::from(u16::MIN as i32)),
                Ok(x) => {
                    assert_eq!(x, u16::MIN)
                }
            );

            // Test that maximum value is OK.
            assert_match!(
                u16::try_from(RObject::from(u16::MAX as i32)),
                Ok(x) => {
                    assert_eq!(x, u16::MAX)
                }
            );

            // Test that some u16 value is OK.
            assert_match!(
                u16::try_from(RObject::from(42)),
                Ok(x) => {
                    assert_eq!(x, 42)
                }
            );

            // Test that R_NaReal is an error.
            assert_match!(
                u16::try_from(RObject::from(R_NaReal)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );

            // Test that some f64 value is an error.
            assert_match!(
                u16::try_from(RObject::from(42.0)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_i32() {
        r_test! {
            // -------------------------------------------------------------------------------------
            // Option::<i32>::try_from tests.
            // -------------------------------------------------------------------------------------

            // Test that R_NaInt is None.
            assert_match!(
                Option::<i32>::try_from(RObject::from(R_NaInt)),
                Ok(None) => {}
            );

            // Test that minimum value is OK.
            assert_match!(
                Option::<i32>::try_from(RObject::from(i32::MIN + 1)),
                Ok(Some(x)) => {
                    assert_eq!(x, i32::MIN + 1)
                }
            );

            // Test that maximum value is OK.
            assert_match!(
                Option::<i32>::try_from(RObject::from(i32::MAX)),
                Ok(Some(x)) => {
                    assert_eq!(x, i32::MAX)
                }
            );

            // Test that some i32 value is OK.
            assert_match!(
                Option::<i32>::try_from(RObject::from(42)),
                Ok(Some(x)) => {
                    assert_eq!(x, 42)
                }
            );

            // Test that R_NaReal is an error.
            assert_match!(
                Option::<i32>::try_from(RObject::from(R_NaReal)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );

            // Test that some f64 value is an error.
            assert_match!(
                Option::<i32>::try_from(RObject::from(42.0)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );

            // -------------------------------------------------------------------------------------
            // i32::try_from tests.
            // -------------------------------------------------------------------------------------

            // Test that R_NaInt is an error.
            assert_match!(
                i32::try_from(RObject::from(R_NaInt)),
                Err(Error::MissingValueError) => {}
            );

            // Test that minimum value is OK.
            assert_match!(
                i32::try_from(RObject::from(i32::MIN + 1)),
                Ok(x) => {
                    assert_eq!(x, i32::MIN + 1)
                }
            );

            // Test that maximum value is OK.
            assert_match!(
                i32::try_from(RObject::from(i32::MAX)),
                Ok(x) => {
                    assert_eq!(x, i32::MAX)
                }
            );

            // Test that some i32 value is OK.
            assert_match!(
                i32::try_from(RObject::from(42)),
                Ok(x) => {
                    assert_eq!(x, 42)
                }
            );

            // Test that R_NaReal is an error.
            assert_match!(
                i32::try_from(RObject::from(R_NaReal)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );

            // Test that some f64 value is an error.
            assert_match!(
                i32::try_from(RObject::from(42.0)),
                Err(Error::UnexpectedType(actual_type, expected_types)) => {
                    assert_eq!(actual_type, REALSXP);
                    assert_eq!(expected_types, vec![INTSXP]);
                }
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_f64() {
        r_test! {
            assert_match!(
                Option::<f64>::try_from(RObject::from(R_NaInt)),
                Ok(None) => {}
            );
            assert_match!(
                Option::<f64>::try_from(RObject::from(R_NaReal)),
                Ok(None) => {}
            );
            assert_match!(
                Option::<f64>::try_from(RObject::from(42)),
                Ok(Some(x)) => {
                    assert_eq!(x, 42.0)
                }
            );
            assert_match!(
                Option::<f64>::try_from(RObject::from(42.0)),
                Ok(Some(x)) => {
                    assert_eq!(x, 42.0)
                }
            );

            assert_match!(
                f64::try_from(RObject::from(R_NaInt)),
                Err(Error::MissingValueError) => {}
            );
            assert_match!(
                f64::try_from(RObject::from(R_NaReal)),
                Err(Error::MissingValueError) => {}
            );
            assert_match!(
                f64::try_from(RObject::from(42)),
                Ok(x) => {
                    assert_eq!(x, 42.0)
                }
            );
            assert_match!(
                f64::try_from(RObject::from(42.0)),
                Ok(x) => {
                    assert_eq!(x, 42.0)
                }
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_Option_String() {
        r_test! {
            let s = RObject::from("abc");

            assert_match!(
                Option::<String>::try_from(s),
                Ok(Some(x)) => {
                    assert_eq!(x, "abc");
                }
            );

            let s = RObject::from("abc");
            SET_STRING_ELT(*s, 0, R_NaString);
            assert_match!(
                Option::<String>::try_from(s),
                Ok(None) => {}
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_String() {
        r_test! {
            let s = RObject::from("abc");

            assert_match!(
                String::try_from(s),
                Ok(x) => {
                    assert_eq!(x, "abc");
                }
            );

            let s = RObject::from("abc");
            SET_STRING_ELT(*s, 0, R_NaString);
            assert_match!(
                String::try_from(s),
                Err(Error::MissingValueError) => {}
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_hashmap_string() {
        r_test! {
            // Create a map of pizza toppings to their acceptability.
            let mut map = HashMap::<String, String>::new();
            map.insert(String::from("pepperoni"), String::from("OK"));
            map.insert(String::from("sausage"), String::from("OK"));
            map.insert(String::from("pineapple"), String::from("NOT OK"));
            let len = map.len();

            // Ensure we created an object of the same size as the map.
            let robj = RObject::from(map);
            assert_eq!(robj.length(), len as isize);

            // Ensure we can convert the object back into a map with the same values.
            let out: HashMap<String, String> = robj.try_into().unwrap();
            assert_eq!(out.get("pepperoni").unwrap(), "OK");
            assert_eq!(out.get("sausage").unwrap(), "OK");
            assert_eq!(out.get("pineapple").unwrap(), "NOT OK");


            let v = r_parse_eval0("c(x = 'a', y = 'b', z = 'c')", R_ENVS.global).unwrap();
            let out: HashMap<String, String> = v.try_into().unwrap();
            assert_eq!(out["x"], "a"); // duplicated name is ignored and first is kept
            assert_eq!(out["y"], "b");
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_hashmap_i32() {
        r_test! {
            // Create a map of pizza toppings to their acceptability.
            let v = r_parse_eval0("list(x = 1L, y = 2L, x = 3L)", R_ENVS.global).unwrap();
            assert_eq!(v.length(), 3 as isize);

            // Ensure we created an object of the same size as the map.
            let out: HashMap<String, i32> = v.try_into().unwrap();

            // Ensure we can convert the object back into a map with the same values.
            assert_eq!(out["x"], 1); // duplicated name is ignored and first is kept
            assert_eq!(out["y"], 2);

            let v = r_parse_eval0("c(x = 1L, y = 2L, x = 3L)", R_ENVS.global).unwrap();
            let out: HashMap<String, i32> = v.try_into().unwrap();
            assert_eq!(out["x"], 1); // duplicated name is ignored and first is kept
            assert_eq!(out["y"], 2);
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_hashmap_Robject() {
        r_test! {
            // Create a map of pizza toppings to their acceptability.
            let v = r_parse_eval0("list(x = c(1L, 2L), y = c('a', 'b'))", R_ENVS.global).unwrap();
            assert_eq!(v.length(), 2 as isize);

            // Ensure we can convert the object back into a map with the same values.
            let out: HashMap<String, RObject> = v.try_into().unwrap();

            let value: Vec<i32> = out.get("x").unwrap().clone().try_into().unwrap();
            assert_eq!(value, vec![1, 2]);

            let value: Vec<String> = out.get("y").unwrap().clone().try_into().unwrap();
            assert_eq!(value, vec!["a", "b"]);
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_Vec_Option_String() {
        r_test! {
            let s = RObject::from(Rf_allocVector(STRSXP, 2));
            SET_STRING_ELT(*s, 0, r_char!("abc"));
            SET_STRING_ELT(*s, 1, R_NaString);

            assert_match!(
                Vec::<Option<String>>::try_from(s),
                Ok(mut x) => {
                    assert_eq!(x.pop(), Some(None));
                    assert_eq!(x.pop(), Some(Some(String::from("abc"))));
                    assert_eq!(x.pop(), None);
                }
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_Vec_Bool() {
        r_test! {
            // Create a vector of logical values.
            let flags = vec![true, false, true];

            let robj = RObject::try_from(&flags).unwrap();

            // We should get an object of the same length as the flags.
            assert_eq!(robj.length(), flags.len() as isize);

            // The values should match the flags.
            assert!(robj.get_bool(0).unwrap().unwrap());
            assert!(!robj.get_bool(1).unwrap().unwrap());
            assert!(robj.get_bool(2).unwrap().unwrap());
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_Vec_String() {
        r_test! {
            let s = RObject::from(Rf_allocVector(STRSXP, 2));
            SET_STRING_ELT(*s, 0, r_char!("abc"));
            SET_STRING_ELT(*s, 1, R_NaString);

            assert_match!(
                Vec::<String>::try_from(s),
                Err(Error::MissingValueError) => {}
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_Vec_i32() {
        r_test! {
            let i = RObject::from(Rf_allocVector(INTSXP, 2));
            SET_INTEGER_ELT(*i, 0, 42);
            SET_INTEGER_ELT(*i, 1, R_NaInt);

            assert_match!(
                Vec::<i32>::try_from(i),
                Err(Error::MissingValueError) => {}
            );

            let j = RObject::from(Rf_allocVector(INTSXP, 3));
            SET_INTEGER_ELT(*j, 0, 1);
            SET_INTEGER_ELT(*j, 1, 2);
            SET_INTEGER_ELT(*j, 2, 3);

            assert_match!(
                Vec::<i32>::try_from(j),
                Ok(x) => {
                    assert_eq!(x, vec![1i32, 2, 3]);
                }
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_RObject_Vec_RObject() {
        r_test! {
            let v = r_parse_eval0("list(c(1L, NA), c(10L, 20L))", R_ENVS.global).unwrap();
            let w = Vec::<RObject>::try_from(v).unwrap();

            assert_match!(
                Vec::<i32>::try_from(w[0].clone()),
                Err(Error::MissingValueError) => {}
            );
            assert_match!(
                Vec::<i32>::try_from(w[1].clone()),
                Ok(x) => {
                    assert_eq!(x, vec![10i32, 20])
                }
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_tryfrom_Vec_RObject_RObject() {
        r_test! {
            let items_in = vec![
                RObject::from(1),
                RObject::from(2),
                RObject::from(3)
            ];

            // Convert the vector of RObjects into a single RObject (a list).
            let list = RObject::try_from(items_in.clone()).unwrap();
            assert_eq!(list.length(), 3);

            // Now convert back to a vector of RObjects.
            let items_out = Vec::<RObject>::try_from(list).unwrap();
            assert_eq!(items_in, items_out);
        }
    }
}
