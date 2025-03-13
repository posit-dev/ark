use libr::*;

use crate::environment::R_ENVS;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::r_int_get;
use crate::r_int_na;
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

        // This materializes ALTREP compact row names (duckplyr)
        let nrow = df_n_row(list.obj.sexp)? as usize;
        let ncol = df_n_col(list.obj.sexp)? as usize;

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
}

/// Compute the number of columns in a data frame
///
/// This is easy, it's the length of the VECSXP
pub fn df_n_col(x: SEXP) -> crate::Result<i32> {
    if !r_is_data_frame(x) {
        return Err(crate::anyhow!("`x` must be a data frame"));
    }

    r_assert_type(x, &[VECSXP])?;

    match i32::try_from(r_length(x)) {
        Ok(n_col) => Ok(n_col),
        Err(_) => Err(crate::anyhow!(
            "Number of columns of `x` must fit in a `i32`."
        )),
    }
}

/// Strategy used when encountering ALTREP compact rownames when determining the number of
/// rows within a data frame
///
/// This is particularly needed for duckdb, where materializing the ALTREP compact
/// rownames to determine the number of rows would materialize the whole query, which we
/// want to avoid because that defeats the purpose of laziness. In the Variables pane we
/// avoid materializing, but in the Data Explorer we have to materialize.
#[derive(Debug, PartialEq)]
enum AltrepCompactRownamesStrategy {
    Materialize,
    DontMaterialize,
}

/// Compute the number of rows in a data frame
///
/// # Safety
///
/// If `x` is a data frame with an ALTREP compact row names attribute (like in duckplyr),
/// then this function will materialize those row names to be able to determine the size.
/// See [df_n_row_if_known()] if you'd like to bail in that scenario instead.
pub fn df_n_row(x: SEXP) -> crate::Result<i32> {
    // Unwrap safety: The `Materialize` strategy ensures this is never `None`
    Ok(df_n_row_impl(x, AltrepCompactRownamesStrategy::Materialize)?.unwrap())
}

/// Compute the number of rows in a data frame, returning [None] if this isn't possible
/// without materializing ALTREP compact row names (like in duckplyr)
pub fn df_n_row_if_possible(x: SEXP) -> crate::Result<Option<i32>> {
    df_n_row_impl(x, AltrepCompactRownamesStrategy::DontMaterialize)
}

fn df_n_row_impl(
    x: SEXP,
    altrep_compact_rownames_strategy: AltrepCompactRownamesStrategy,
) -> crate::Result<Option<i32>> {
    if !r_is_data_frame(x) {
        return Err(crate::anyhow!("`x` must be a data frame"));
    }

    r_assert_type(x, &[VECSXP])?;

    // We can't go through `Rf_getAttrib()` directly, this materializes ALTREP compact row
    // name objects like in duckplyr. Instead we use `.row_names_info(x, 0)` which goes
    // through `getAttrib0()` and does not materialize.
    let row_names = RFunction::new("base", ".row_names_info")
        .param("x", x)
        .param("type", 0)
        .call_in(R_ENVS.global)?;

    // If the row names object is ALTREP and looks like an instance of compact row names,
    // we can't touch it if [AltrepCompactRownamesStrategy::DontMaterialize] is set.
    if altrep_compact_rownames_strategy == AltrepCompactRownamesStrategy::DontMaterialize &&
        is_likely_altrep_compact_row_names(row_names.sexp)
    {
        return Ok(None);
    }

    // If the row names object is just in compact form like `c(NA, -5)`, we extract the
    // number of rows from that
    if is_compact_row_names(row_names.sexp) {
        return Ok(Some(compact_row_names_n_row(row_names.sexp)));
    }

    // Otherwise the row names object is typically an integer vector or character vector,
    // and we just take the length of that to get the number of rows
    match i32::try_from(r_length(row_names.sexp)) {
        Ok(n_row) => Ok(Some(n_row)),
        Err(_) => Err(crate::anyhow!("Number of rows of `x` must fit in a `i32`.")),
    }
}

/// Is `x` an instance of compact row names?
///
/// These take the form `c(NA, -5L)`, i.e.
/// - INTSXP
/// - Length 2
/// - The first element is `NA`
///
/// The second element will be the row names, typically as a negative value.
fn is_compact_row_names(x: SEXP) -> bool {
    r_typeof(x) == INTSXP && r_length(x) == 2 && r_int_get(x, 0) == r_int_na()
}

/// Is `x` likely an instance of ALTREP compact row names?
///
/// In the case of duckdb, their compact row names object is actually an ALTREP object
/// that knows it is an INTSXP and knows it is length 2, but you can't query any values
/// in the vector, otherwise it will materialize the full duckdb query to be able to
/// return the number of rows. You can't even call `INTEGER_ELT(x, 0)` on this currently,
/// even though in theory only `INTEGER_ELT(x, 1)` should trigger the materialization.
///
/// This means we can't do the full check for compact row names, so we leave off the `NA`
/// check and say that if the object meets the following criteria, it is probably an
/// ALTREP compact row names object and we can't query the number of rows:
/// - ALTREP
/// - INTSXP
/// - Length 2
///
/// TODO: We should ask duckdb to add `ALTREP_ELT` support so that we can query
/// `INTEGER_ELT(x, 0)` without materializing the duckdb query, and then we can do
/// the full [is_compact_row_names()] check!
fn is_likely_altrep_compact_row_names(x: SEXP) -> bool {
    r_is_altrep(x) && r_typeof(x) == INTSXP && r_length(x) == 2
}

fn compact_row_names_n_row(x: SEXP) -> i32 {
    i32::abs(r_int_get(x, 1))
}

#[cfg(test)]
mod tests {
    use stdext::assert_match;

    use crate::r_alloc_integer;
    use crate::r_chr_poke;
    use crate::r_list_poke;
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
            df.set_attr("names", RObject::null().sexp);
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
            let nms = df.attr("names").unwrap();
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

    // TODO: This is a very heavy test.
    //
    // - It requires `duckplyr` and `duckdb` as dependencies.
    // - It greatly modifies the global state, as duckplyr shims dplyr verbs.
    // - It relies on `duckdb` internals to check ALTREP materialization.
    //
    // When we switch to nextest with 1 R session per test, we should consider running
    // this test in CI, but we should not feel too much pressure to run it if it gives us
    // trouble.
    //
    //     #[test]
    //     fn test_duckplyr_not_materialized() {
    //         use crate::df_n_row;
    //         use crate::df_n_row_if_known;
    //         use crate::exec::RFunction;
    //         use crate::exec::RFunctionExt;
    //         use crate::fixtures::package::package_is_installed;
    //
    //         crate::r_task(|| {
    //             if !package_is_installed("duckplyr") {
    //                 return;
    //             }
    //
    //             // Turn off autoupload startup message
    //             harp::parse_eval_global("Sys.setenv(DUCKPLYR_FALLBACK_AUTOUPLOAD = 0)").unwrap();
    //
    //             let df = harp::parse_eval_global("duckplyr::duckdb_tibble(x = 1:100)").unwrap();
    //
    //             // Should not be able to compute `n_row` with `df_n_row_if_known()`
    //             let n_row = df_n_row_if_known(df.sexp).expect("Can return `None` without `Error`");
    //             assert!(n_row.is_none());
    //
    //             // And `df` should not be materialized
    //             // This relies on duckdb internals
    //             let is_materialized = unsafe {
    //                 RFunction::new_internal("duckdb", "df_is_materialized")
    //                     .param("df", df)
    //                     .call()
    //                     .unwrap()
    //                     .to::<bool>()
    //                     .unwrap()
    //             };
    //             assert!(!is_materialized);
    //
    //             let df = harp::parse_eval_global("duckplyr::duckdb_tibble(x = 1:100)").unwrap();
    //
    //             // Should be able to compute `n_row` with `df_n_row()`
    //             let n_row = df_n_row(df.sexp).expect("Can compute `n_row`");
    //             assert_eq!(n_row, 100);
    //
    //             // And `df` should be materialized
    //             // This relies on duckdb internals
    //             let is_materialized = unsafe {
    //                 RFunction::new_internal("duckdb", "df_is_materialized")
    //                     .param("df", df)
    //                     .call()
    //                     .unwrap()
    //                     .to::<bool>()
    //                     .unwrap()
    //             };
    //             assert!(is_materialized);
    //         })
    //     }
}
