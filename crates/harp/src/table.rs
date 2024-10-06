use libr::*;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::r_length;
use crate::object::RObject;
use crate::r_assert_type;
use crate::utils::r_is_data_frame;
use crate::utils::r_is_matrix;
use crate::utils::r_typeof;
use crate::vector::CharacterVector;
use crate::vector::Vector;

#[derive(Clone, Copy)]
pub enum TableKind {
    Dataframe,
    Matrix,
}

pub struct TableInfo {
    pub kind: TableKind,
    pub dims: TableDim,
    pub col_names: ColumnNames,
}

// TODO: Might want to encode as types with methods so that we can make
// assumptions about memory layout more safely. Also makes it possible
// to compute properties more lazily.
pub fn table_info(x: SEXP) -> Option<TableInfo> {
    if r_is_data_frame(x) {
        return df_info(x).ok();
    }

    if r_is_matrix(x) {
        return mat_info(x).ok();
    }

    None
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

pub fn df_info(x: SEXP) -> anyhow::Result<TableInfo> {
    unsafe {
        let dims = df_dim(x)?;
        let col_names = ColumnNames::new(Rf_getAttrib(x, R_NamesSymbol));

        Ok(TableInfo {
            kind: TableKind::Dataframe,
            dims,
            col_names,
        })
    }
}

pub fn mat_info(x: SEXP) -> anyhow::Result<TableInfo> {
    let dims = mat_dim(x);

    let col_names = RFunction::from("colnames").add(x).call()?;
    let col_names = ColumnNames::new(col_names.sexp);

    Ok(TableInfo {
        kind: TableKind::Matrix,
        dims,
        col_names,
    })
}

pub struct TableDim {
    pub num_rows: i32,
    pub num_cols: i32,
}

/// Safety: Assumes a data frame as input.
/// TODO: Extract row info from attribute.
pub unsafe fn df_dim(data: SEXP) -> harp::Result<TableDim> {
    // FIXME: We shouldn't dispatch to methods here
    let dims = RFunction::new("base", "dim.data.frame")
        .add(data)
        .call()
        .unwrap();

    let Ok(_) = r_assert_type(dims.sexp, &[libr::INTSXP]) else {
        return Err(harp::anyhow!(
            "Data frame dimensions must be an integer vector, instead it has type `{}`",
            harp::r_type2char(dims.kind())
        ));
    };
    if dims.length() != 2 {
        return Err(harp::anyhow!(
            "Data frame must have 2 dimensions, instead it has {}",
            dims.length()
        ));
    }

    Ok(TableDim {
        num_rows: INTEGER_ELT(dims.sexp, 0),
        num_cols: INTEGER_ELT(dims.sexp, 1),
    })
}

pub fn mat_dim(x: SEXP) -> TableDim {
    unsafe {
        let dims = Rf_getAttrib(x, R_DimSymbol);

        // Might want to return an error instead, or take a strongly typed input
        if r_typeof(dims) != INTSXP || r_length(dims) != 2 {
            return TableDim {
                num_rows: r_length(x) as i32,
                num_cols: 1,
            };
        }

        TableDim {
            num_rows: INTEGER_ELT(dims, 0),
            num_cols: INTEGER_ELT(dims, 1),
        }
    }
}

pub struct ColumnNames {
    pub names: Option<CharacterVector>,
}

impl ColumnNames {
    pub fn new(names: SEXP) -> Self {
        unsafe {
            let names = if r_typeof(names) == STRSXP {
                Some(CharacterVector::new_unchecked(names))
            } else {
                None
            };
            Self { names }
        }
    }

    pub fn get_unchecked(&self, index: isize) -> Option<String> {
        if let Some(names) = &self.names {
            if let Some(name) = names.get_unchecked(index) {
                if name.len() > 0 {
                    return Some(name);
                }
            }
        }
        None
    }
}
