use libr::*;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::object::RObject;
use crate::utils::r_is_data_frame;
use crate::utils::r_is_matrix;
use crate::utils::r_typeof;
use crate::vector::CharacterVector;
use crate::vector::Vector;

#[derive(Debug, Clone)]
pub enum TableKind {
    Dataframe,
    Matrix,
}

#[derive(Debug, Clone)]
pub struct TableInfo {
    pub kind: TableKind,
    pub dims: TableDim,
    pub col_names: ColumnNames,
    data: RObject,
}

impl IntoIterator for TableInfo {
    type Item = TableInfoIterator;
    type IntoIter = TableInfoIterator;

    fn into_iter(self) -> TableInfoIterator {
        TableInfoIterator {
            index: 0,
            kind: self.kind,
            num_cols: self.dims.num_cols as isize,
            col_names: self.col_names,
            data: self.data,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TableInfoIterator {
    index: isize,
    kind: TableKind,
    num_cols: isize,
    col_names: ColumnNames,
    data: RObject,
}

impl Iterator for TableInfoIterator {
    type Item = TableInfoIterator;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.num_cols {
            self.index += 1;
            Some(self.clone())
        } else {
            None
        }
    }
}

impl TableInfoIterator {
    pub fn name(&self) -> Option<String> {
        self.col_names.get_unchecked(self.index)
    }
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
        let col_names = ColumnNames::new(Rf_getAttrib(x, R_NamesSymbol));

        Ok(TableInfo {
            kind: TableKind::Dataframe,
            dims,
            col_names,
            data: x.into(),
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
        data: x.into(),
    })
}

#[derive(Debug, Clone)]
pub struct TableDim {
    pub num_rows: i32,
    pub num_cols: i32,
}

pub fn df_dim(data: SEXP) -> TableDim {
    unsafe {
        let dims = RFunction::new("base", "dim.data.frame")
            .add(data)
            .call()
            .unwrap();

        TableDim {
            num_rows: INTEGER_ELT(dims.sexp, 0),
            num_cols: INTEGER_ELT(dims.sexp, 1),
        }
    }
}

pub fn mat_dim(x: SEXP) -> TableDim {
    unsafe {
        let dims = Rf_getAttrib(x, R_DimSymbol);

        TableDim {
            num_rows: INTEGER_ELT(dims, 0),
            num_cols: INTEGER_ELT(dims, 1),
        }
    }
}

#[derive(Debug, Clone)]
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
