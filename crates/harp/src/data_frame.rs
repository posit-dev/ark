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

        let Some(dim) = list.obj.dim()? else {
            return Err(harp::anyhow!("Data frame doesn't have dimensions"));
        };

        if dim.len() != 2 {
            return Err(harp::anyhow!(
                "Data frame must have 2 dimensions, instead it has {}",
                dim.len()
            ));
        }
        let nrow = *dim.get(0).unwrap();
        let ncol = *dim.get(1).unwrap();

        // SAFETY: Protected by `list`
        let obj = RObject::view(sexp);

        let Some(names) = obj.names() else {
            return Err(harp::anyhow!("Data frame doesn't have names"));
        };
        let Ok(names) = harp::assert_non_optional(names) else {
            return Err(harp::anyhow!("Data frame has missing names"));
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
