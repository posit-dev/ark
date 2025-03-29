use libr::*;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::RObject;
use crate::utils::r_is_data_frame;
use crate::utils::r_is_matrix;

#[derive(Clone, Copy)]
pub enum TableKind {
    Dataframe,
    Matrix,
}

pub fn table_kind(x: SEXP) -> Option<TableKind> {
    if r_is_data_frame(x) {
        Some(TableKind::Dataframe)
    } else if r_is_matrix(x) {
        Some(TableKind::Matrix)
    } else {
        None
    }
}

/// Extracts a single column from a table.
///
/// - `x` - The table to extract the column from.
/// - `column_index` - The index of the column to extract (0-based)
/// - `kind` - The kind of table `x` is (matrix or data frame).
///
pub fn tbl_get_column(x: SEXP, column_index: i32, kind: TableKind) -> anyhow::Result<RObject> {
    // Get the column to sort by
    match kind {
        TableKind::Dataframe => {
            let column = RFunction::new("base", "[[")
                .add(x)
                .add(RObject::from(column_index + 1))
                .call()?;
            Ok(column)
        },
        TableKind::Matrix => {
            let column = RFunction::new("base", "[")
                .add(x)
                .add(unsafe { R_MissingArg })
                .add(RObject::from(column_index + 1))
                .call()?;
            Ok(column)
        },
    }
}
