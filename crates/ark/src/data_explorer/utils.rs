use amalthea::comm::data_explorer_comm::ColumnDisplayType;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_inherits;
use harp::utils::r_is_object;
use harp::utils::r_is_s4;
use harp::utils::r_typeof;
use libr::*;

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

// This returns the type of an _element_ of the column. In R atomic
// vectors do not have a distinct internal type but we pretend that they
// do for the purpose of integrating with Positron types.
pub fn display_type(x: SEXP) -> ColumnDisplayType {
    if r_is_s4(x) {
        return ColumnDisplayType::Unknown;
    }

    if r_is_object(x) {
        // `haven_labelled` objects inherit from their internal data type
        // such as integer or character. We special case them here before
        // checking the internal types below.
        if r_inherits(x, "haven_labelled") {
            return ColumnDisplayType::String;
        }

        if r_inherits(x, "logical") {
            return ColumnDisplayType::Boolean;
        }

        if r_inherits(x, "integer") {
            return ColumnDisplayType::Number;
        }
        if r_inherits(x, "double") {
            return ColumnDisplayType::Number;
        }
        if r_inherits(x, "complex") {
            return ColumnDisplayType::Number;
        }
        if r_inherits(x, "numeric") {
            return ColumnDisplayType::Number;
        }

        if r_inherits(x, "character") {
            return ColumnDisplayType::String;
        }
        if r_inherits(x, "factor") {
            return ColumnDisplayType::String;
        }

        if r_inherits(x, "Date") {
            return ColumnDisplayType::Date;
        }
        if r_inherits(x, "POSIXct") {
            return ColumnDisplayType::Datetime;
        }
        if r_inherits(x, "POSIXlt") {
            return ColumnDisplayType::Datetime;
        }

        // TODO: vctrs's list_of
        if r_inherits(x, "list") {
            return ColumnDisplayType::Unknown;
        }

        // Catch-all, including for data frame
        return ColumnDisplayType::Unknown;
    }

    match r_typeof(x) {
        LGLSXP => return ColumnDisplayType::Boolean,
        INTSXP | REALSXP | CPLXSXP => return ColumnDisplayType::Number,
        STRSXP => return ColumnDisplayType::String,
        VECSXP => return ColumnDisplayType::Unknown,
        _ => return ColumnDisplayType::Unknown,
    }
}
