use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use libr::SEXP;

use crate::modules::ARK_ENVS;

pub fn tbl_subset_with_view_indices(
    x: SEXP,
    view_indices: &Option<Vec<i32>>,
    i: Option<Vec<i64>>,
    j: Option<Vec<i64>>,
) -> anyhow::Result<RObject> {
    let i = match view_indices {
        Some(view_indices) => match i {
            Some(i) => Some(i.iter().map(|i| view_indices[*i as usize] as i64).collect()),
            None => None,
        },
        None => match i {
            Some(i) => Some(i.iter().map(|i| i + 1).collect()),
            None => None,
        },
    };
    let j = match j {
        Some(j) => Some(j.iter().map(|j| j + 1).collect()),
        None => None,
    };
    tbl_subset(x, i, j)
}

fn tbl_subset(x: SEXP, i: Option<Vec<i64>>, j: Option<Vec<i64>>) -> anyhow::Result<RObject> {
    let mut call = RFunction::from(".ps.table_subset");
    call.param("x", x);
    if let Some(i) = i {
        call.param("i", &i);
    }
    if let Some(j) = j {
        call.param("j", &j);
    }

    Ok(call.call_in(ARK_ENVS.positron_ns)?)
}
