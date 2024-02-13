use libr::*;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::RObject;
use crate::utils::r_is_data_frame;
use crate::utils::r_is_matrix;
use crate::utils::r_typeof;
use crate::vector::CharacterVector;
use crate::vector::Vector;

pub enum TableKind {
    Dataframe,
    Matrix,
}

pub struct TableInfo {
    pub kind: TableKind,
    pub num_rows: i64,
    pub num_cols: i32,
    pub col_names: ColumnNames,
}

pub fn table_info(x: SEXP) -> anyhow::Result<TableInfo> {
    if r_is_data_frame(x) {
        return df_info(x);
    }

    if r_is_matrix(x) {
        return mat_info(x);
    }

    // TODO: better error message
    anyhow::bail!("Unsupported type for data viewer");
}

pub fn df_info(x: SEXP) -> anyhow::Result<TableInfo> {
    unsafe {
        let dims = df_dim(x);

        let col_names = RObject::new(Rf_getAttrib(x, R_NamesSymbol));
        let col_names = ColumnNames::new(col_names.sexp);

        Ok(TableInfo {
            kind: TableKind::Dataframe,
            num_rows: dims.nrow as i64,
            num_cols: dims.ncol,
            col_names,
        })
    }
}

pub fn mat_info(x: SEXP) -> anyhow::Result<TableInfo> {
    unsafe {
        let dims = RObject::new(Rf_getAttrib(x, R_DimSymbol));
        let num_rows = INTEGER_ELT(dims.sexp, 0) as i64;
        let num_cols = INTEGER_ELT(dims.sexp, 1);

        let col_names = RFunction::from("colnames").add(x).call()?;
        let col_names = ColumnNames::new(col_names.sexp);

        Ok(TableInfo {
            kind: TableKind::Matrix,
            num_rows,
            num_cols,
            col_names,
        })
    }
}

pub struct DataFrameDim {
    pub nrow: i32,
    pub ncol: i32,
}

pub fn df_dim(data: SEXP) -> DataFrameDim {
    unsafe {
        let dim = RFunction::new("base", "dim.data.frame")
            .add(data)
            .call()
            .unwrap();

        DataFrameDim {
            nrow: INTEGER_ELT(*dim, 0),
            ncol: INTEGER_ELT(*dim, 1),
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
