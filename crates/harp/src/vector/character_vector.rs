//
// character_vector.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::os::raw::c_char;

use libr::cetype_t_CE_UTF8;
use libr::R_NaString;
use libr::R_xlen_t;
use libr::Rf_mkCharLenCE;
use libr::SET_STRING_ELT;
use libr::SEXP;
use libr::STRING_ELT;
use libr::STRSXP;

use crate::object::RObject;
use crate::r_assert_type;
use crate::r_chr_poke;
use crate::utils::r_str_to_owned_utf8_unchecked;
use crate::vector::FormatOptions;
use crate::vector::Vector;

#[harp_macros::vector]
pub struct CharacterVector {
    pub object: RObject,
}

impl CharacterVector {
    pub fn slice(&self) -> &[SEXP] {
        unsafe {
            let data = harp::chr_cbegin(self.object.sexp);
            std::slice::from_raw_parts(data, self.len())
        }
    }
}

impl Vector for CharacterVector {
    type Item = str;
    type Type = String;
    const SEXPTYPE: u32 = STRSXP;
    type UnderlyingType = SEXP;
    type CompareType = &'static str;

    fn data(&self) -> SEXP {
        self.object.sexp
    }

    unsafe fn new_unchecked(object: impl Into<SEXP>) -> Self {
        Self {
            object: RObject::new(object.into()),
        }
    }

    fn create<T>(data: T) -> Self
    where
        T: IntoIterator,
        <T as IntoIterator>::IntoIter: ExactSizeIterator,
        <T as IntoIterator>::Item: AsRef<Self::Item>,
    {
        unsafe {
            // convert into iterator
            let mut data = data.into_iter();

            // build our character vector
            let n = data.len();
            let vector = CharacterVector::with_length(n);
            for i in 0..data.len() {
                let value = data.next().unwrap_unchecked();
                let value = value.as_ref();
                let charsexp = Rf_mkCharLenCE(
                    value.as_ptr() as *const c_char,
                    value.len() as i32,
                    cetype_t_CE_UTF8,
                );
                SET_STRING_ELT(vector.data(), i as R_xlen_t, charsexp);
            }

            vector
        }
    }

    fn is_na(x: &Self::UnderlyingType) -> bool {
        unsafe { *x == R_NaString }
    }

    fn get_unchecked_elt(&self, index: isize) -> Self::UnderlyingType {
        unsafe { STRING_ELT(self.data(), index as R_xlen_t) }
    }

    fn convert_value(x: &Self::UnderlyingType) -> Self::Type {
        r_str_to_owned_utf8_unchecked(*x)
    }

    fn format_one(&self, x: Self::Type, options: Option<&FormatOptions>) -> String {
        if let Some(&FormatOptions { quote, .. }) = options {
            if quote {
                format!(r#""{}""#, x.replace('"', r#"\""#))
            } else {
                x
            }
        } else {
            x
        }
    }
}

impl TryFrom<&[SEXP]> for CharacterVector {
    type Error = harp::Error;

    fn try_from(value: &[SEXP]) -> harp::Result<Self> {
        unsafe {
            let vec = Self::with_length(value.len());
            let sexp = vec.object.sexp;

            for (i, elt) in value.into_iter().enumerate() {
                r_assert_type(*elt, &[libr::CHARSXP])?;
                r_chr_poke(sexp, i as R_xlen_t, *elt);
            }

            Ok(vec)
        }
    }
}

impl TryFrom<&CharacterVector> for Vec<String> {
    type Error = harp::Error;

    fn try_from(value: &CharacterVector) -> harp::Result<Self> {
        super::try_vec_from_r_vector(value)
    }
}

#[cfg(test)]
mod test {
    use libr::STRSXP;

    use crate::r_test;
    use crate::utils::r_typeof;
    use crate::vector::*;

    #[test]
    fn test_character_vector() {
        r_test! {

            let vector = CharacterVector::create(&["hello", "world"]);
            assert!(vector == ["hello", "world"]);
            assert!(vector == &["hello", "world"]);

            let mut it = vector.iter();

            assert_eq!(it.next(), Some(Some(String::from("hello"))));
            assert_eq!(it.next(), Some(Some(String::from("world"))));
            assert!(it.next().is_none());

            let vector = CharacterVector::create([
                "hello".to_string(),
                "world".to_string()
            ]);
            assert!(vector == ["hello", "world"]);
            assert!(vector == &["hello", "world"]);

            assert!(vector.get_unchecked(0) == Some(String::from("hello")));
            assert!(vector.get_unchecked(1) == Some(String::from("world")));

        }
    }

    #[test]
    fn test_create() {
        r_test! {

            let expected = ["Apple", "Orange", "한"];
            let vector = CharacterVector::create(&expected);
            assert_eq!(vector.get(0).unwrap(), Some(String::from("Apple")));
            assert_eq!(vector.get(1).unwrap(), Some(String::from("Orange")));
            assert_eq!(vector.get(2).unwrap(), Some(String::from("한")));

            let alphabet = ["a", "b", "c"];

            // &[&str]
            let s = CharacterVector::create(&alphabet);
            assert_eq!(r_typeof(*s), STRSXP);
            assert_eq!(s, alphabet);

            // &[&str; N]
            let s = CharacterVector::create(&alphabet[..]);
            assert_eq!(r_typeof(*s), STRSXP);
            assert_eq!(s, alphabet);

            // Vec<String>
            let s = CharacterVector::create(alphabet.to_vec());
            assert_eq!(r_typeof(*s), STRSXP);
            assert_eq!(s, alphabet);

        }
    }
}
