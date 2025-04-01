use libr::*;

use crate::r_length;
use crate::utils::*;
use crate::vector::Vector;
use crate::List;
use crate::RObject;

/// Typed data frame
///
/// Type guarantees:
/// - Storage type is list.
/// - Class is `"data.frame"`.
/// - All columns are vectors and the same size as the data frame.
#[derive(Debug)]
pub struct DataFrame {
    pub list: List,
    pub obj: RObject,

    pub names: Vec<String>,
    pub nrow: usize,
    pub ncol: usize,
}

impl DataFrame {
    pub fn new(sexp: libr::SEXP) -> harp::Result<Self> {
        let list = List::new(sexp)?;
        harp::assert_class(sexp, "data.frame")?;

        // This materializes ALTREP compact row names (duckplyr) and we are okay with
        // that. If you just need the number of columns without full validation, use
        // the static method [DataFrame::n_col()].
        let nrow = Self::n_row(list.obj.sexp)? as usize;
        let ncol = Self::n_col(list.obj.sexp)? as usize;

        let Some(names) = list.obj.names() else {
            return Err(harp::anyhow!("Data frame must have names"));
        };
        let Ok(names) = harp::assert_non_optional(names) else {
            return Err(harp::anyhow!("Data frame can't have missing names"));
        };

        // Validate columns
        for obj in list.iter() {
            let obj = RObject::view(obj);

            if unsafe { libr::Rf_isVector(obj.sexp) == 0 } {
                return Err(harp::anyhow!("Data frame column must be a vector"));
            }

            if obj.length() as usize != nrow {
                return Err(harp::anyhow!(
                    "Data frame column must be the same size as the number of rows"
                ));
            }
        }

        // SAFETY: Protected by `list`
        let obj = RObject::view(sexp);

        Ok(Self {
            list,
            obj,
            names,
            nrow,
            ncol,
        })
    }

    pub fn col(&self, name: &str) -> harp::Result<RObject> {
        let Some(idx) = self.names.iter().position(|n| n == name) else {
            return Err(harp::Error::MissingColumnError { name: name.into() });
        };

        self.list
            // FIXME: `get()` should take `usize`
            .get(idx as isize)?
            .ok_or_else(|| harp::unreachable!("missing column"))
    }

    /// Compute the number of columns of a data frame
    ///
    /// # Notes
    ///
    /// In general, prefer [DataFrame::new()] followed by accessing the `ncol` field,
    /// as that validates the data frame on the way in. Use this static method if you
    /// need maximal performance, or if you only need the number of columns, and computing
    /// the number of rows would materialize ALTREP objects unnecessarily.
    pub fn n_col(x: libr::SEXP) -> crate::Result<i32> {
        if !r_is_data_frame(x) {
            return Err(crate::anyhow!("`x` must be a data frame"));
        }

        match i32::try_from(r_length(x)) {
            Ok(n_col) => Ok(n_col),
            Err(_) => Err(crate::anyhow!(
                "Number of columns of `x` must fit in a `i32`."
            )),
        }
    }

    /// Compute the number of rows of a data frame
    ///
    /// # Notes
    ///
    /// In general, prefer [DataFrame::new()] followed by accessing the `nrow` field,
    /// as that validates the data frame on the way in. Use this static method if you
    /// need maximal performance.
    pub fn n_row(x: SEXP) -> crate::Result<i32> {
        if !r_is_data_frame(x) {
            return Err(crate::anyhow!("`x` must be a data frame"));
        }

        // Note that this turns compact row names of the form `c(NA, -5)` into ALTREP compact
        // intrange objects. This is fine for our purposes because the row names are never
        // fully expanded as we determine their length.
        //
        // There is a special case with duckplyr where the row names object can be an ALTREP
        // integer vector that looks like an instance of compact row names like `c(NA, -5)`.
        // Touching this with `INTEGER()` or `INTEGER_ELT()` to determine the number of rows
        // will materialize the whole query (and run arbitrary R code). We've determined the
        // only maintainable strategy for classes like this is to provide higher level ark
        // hooks where packages like duckplyr can intercede before we even get here, providing
        // their own custom methods (like for the variables pane). That keeps our hot path
        // simpler, as we unconditionally materialize ALTREP vectors, while still providing a
        // way to opt out.
        let row_names = RObject::view(x).get_attribute_row_names();

        let Some(row_names) = row_names else {
            return Err(crate::anyhow!("`x` must have row names"));
        };

        // The row names object is typically an integer vector (possibly ALTREP compact
        // intrange that knows its length) or character vector, and we just take the length of
        // that to get the number of rows
        match i32::try_from(r_length(row_names.sexp)) {
            Ok(n_row) => Ok(n_row),
            Err(_) => Err(crate::anyhow!("Number of rows of `x` must fit in a `i32`.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use stdext::assert_match;

    use crate::r_alloc_integer;
    use crate::r_chr_poke;
    use crate::r_list_poke;
    use crate::r_null;
    use crate::vector::Vector;
    use crate::DataFrame;
    use crate::List;
    use crate::RObject;

    #[test]
    fn test_data_frame_structure() {
        crate::r_task(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            let df = DataFrame::new(df.sexp).unwrap();

            assert_match!(df.list, List { .. });
            assert_match!(df.obj, RObject { .. });

            assert_eq!(df.names, vec![String::from("x"), String::from("y")]);
            assert_eq!(df.nrow, 2);
            assert_eq!(df.ncol, 2);
        })
    }

    #[test]
    fn test_data_frame_no_names() {
        crate::r_task(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            df.set_attribute("names", r_null());
            let df = DataFrame::new(df.sexp);

            assert_match!(df, harp::Result::Err(err) => {
                assert!(format!("{err}").contains("must have names"))
            });
        })
    }

    #[test]
    fn test_data_frame_bad_names() {
        crate::r_task(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            let nms = df.get_attribute("names").unwrap();
            unsafe {
                r_chr_poke(nms.sexp, 0, libr::R_NaString);
            }
            let df = DataFrame::new(df.sexp);

            assert_match!(df, harp::Result::Err(err) => {
                assert!(format!("{err}").contains("missing names"))
            });
        })
    }

    #[test]
    fn test_data_frame_bad_column_type() {
        crate::r_task(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            r_list_poke(df.sexp, 0, RObject::null().sexp);

            let df = DataFrame::new(df.sexp);

            assert_match!(df, harp::Result::Err(err) => {
                assert!(format!("{err}").contains("must be a vector"))
            });
        })
    }

    #[test]
    fn test_data_frame_bad_column_size() {
        crate::r_task(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            let bad_col = r_alloc_integer(3);
            r_list_poke(df.sexp, 0, bad_col);

            let df = DataFrame::new(df.sexp);

            assert_match!(df, harp::Result::Err(err) => {
                assert!(format!("{err}").contains("must be the same size"))
            });
        })
    }

    #[test]
    fn test_data_frame_col() {
        crate::r_task(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            let df = DataFrame::new(df.sexp).unwrap();

            let col_y = df.col("y").unwrap();
            assert_eq!(col_y.sexp, df.list.get_value(1).unwrap().sexp);

            assert_match!(df.col("foo"), harp::Result::Err(err) => {
                assert_match!(err, harp::Error::MissingColumnError { ref name } => {
                    assert_eq!(name, "foo");
                });
                assert!(format!("{err}").contains("Can't find column `foo` in data frame"))
            });
        })
    }
}
