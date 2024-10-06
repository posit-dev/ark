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

        let dim = unsafe { harp::df_dim(list.obj.sexp) }?;
        let nrow = dim.num_rows as usize;
        let ncol = list.obj.length() as usize;

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
}
