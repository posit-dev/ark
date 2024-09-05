use crate::vector::Vector;
use crate::List;
use crate::RObject;

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
        let list = unsafe { List::new(sexp) }?;
        harp::assert_class(sexp, "data.frame")?;

        // SAFETY: Protected by `list`
        let obj = RObject::view(sexp);

        let dim = unsafe { harp::df_dim(obj.sexp) }?;
        let nrow = dim.num_rows as usize;
        let ncol = list.obj.length() as usize;

        let Some(names) = obj.names() else {
            return Err(harp::unexpected_structure!("Data frame must have names"));
        };
        let Ok(names) = harp::assert_non_optional(names) else {
            return Err(harp::unexpected_structure!(
                "Data frame can't have missing names"
            ));
        };

        Ok(Self {
            list,
            obj,
            names,
            nrow,
            ncol,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::assert_match;
    use crate::r_chr_poke;
    use crate::test::r_test;
    use crate::DataFrame;
    use crate::List;
    use crate::RObject;

    #[test]
    fn test_data_frame_structure() {
        r_test(|| {
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
        r_test(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            df.set_attr("names", RObject::null().sexp);
            let df = DataFrame::new(df.sexp);

            assert_match!(df, harp::Result::Err(err) => {
                assert_match!(err, harp::Error::UnexpectedStructure(..));
                assert!(format!("{err}").contains("must have names"))
            });
        })
    }

    #[test]
    fn test_data_frame_bad_names() {
        r_test(|| {
            let df = harp::parse_eval_base("data.frame(x = 1:2, y = 3:4)").unwrap();
            let nms = df.attr("names").unwrap();
            unsafe {
                r_chr_poke(nms.sexp, 0, libr::R_NaString);
            }
            let df = DataFrame::new(df.sexp);

            assert_match!(df, harp::Result::Err(err) => {
                assert_match!(err, harp::Error::UnexpectedStructure(..));
                assert!(format!("{err}").contains("missing names"))
            });
        })
    }
}
