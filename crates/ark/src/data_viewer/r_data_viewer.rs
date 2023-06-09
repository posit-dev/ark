//
// r-data-viewer.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use anyhow::bail;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_lock;
use harp::utils::r_assert_length;
use harp::utils::r_assert_type;
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_is_simple_vector;
use harp::utils::r_typeof;
use harp::utils::r_xlength;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_sys::R_CallMethodDef;
use libR_sys::R_DimSymbol;
use libR_sys::R_MissingArg;
use libR_sys::R_NamesSymbol;
use libR_sys::R_NilValue;
use libR_sys::R_RowNamesSymbol;
use libR_sys::Rf_getAttrib;
use libR_sys::INTEGER_ELT;
use libR_sys::SEXP;
use libR_sys::STRSXP;
use libR_sys::VECTOR_ELT;
use serde::Deserialize;
use serde::Serialize;
use stdext::spawn;
use uuid::Uuid;

use crate::lsp::globals::comm_manager_tx;

pub struct RDataViewer {
    pub id: String,
    pub title: String,
    pub data: RObject,
    pub comm: CommSocket,
}

#[derive(Deserialize, Serialize)]
pub struct DataColumn {
    pub name: String,

    #[serde(rename = "type")]
    pub column_type: String,

    pub data: Vec<String>,
}

#[derive(Deserialize, Serialize)]
pub struct DataSet {
    pub id: String,
    pub title: String,
    pub columns: Vec<DataColumn>,

    #[serde(rename = "rowCount")]
    pub row_count: isize,
}

struct ColumnNames {
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

impl DataSet {
    unsafe fn extract_columns(
        object: SEXP,
        prefix: Option<String>,
        row_count: isize,
        columns: &mut Vec<DataColumn>,
    ) -> Result<(), anyhow::Error> {
        if r_is_data_frame(object) {
            unsafe {
                let names = ColumnNames::new(Rf_getAttrib(object, R_NamesSymbol));

                let n_columns = r_xlength(object);
                for i in 0..n_columns {
                    let col_name = names.get_unchecked(i);

                    let name = match prefix {
                        None => match col_name {
                            Some(name) => name,
                            None => format!("[, {}]", i + 1),
                        },

                        Some(ref prefix) => match col_name {
                            Some(name) => format!("{}${}", prefix, name),
                            None => format!("{}[, {}]", prefix, i + 1),
                        },
                    };

                    // Protecting with `RObject` in case `object` happens to be an ALTLIST
                    let column = RObject::from(VECTOR_ELT(object, i as isize));
                    Self::extract_columns(*column, Some(name), row_count, columns)?;
                }
            }
        } else if r_is_matrix(object) {
            unsafe {
                let dim = Rf_getAttrib(object, R_DimSymbol);
                let n_columns = INTEGER_ELT(dim, 1) as isize;
                let n_rows = INTEGER_ELT(dim, 0) as isize;
                if n_rows != row_count {
                    bail!("matrix column with incompatible number of rows");
                }

                let colnames = RFunction::from("colnames").add(object).call()?;
                let colnames = ColumnNames::new(*colnames);

                for i in 0..n_columns {
                    let col_name = colnames.get_unchecked(i);

                    let name = match prefix {
                        None => match col_name {
                            Some(name) => name,
                            None => format!("[, {}]", i + 1),
                        },
                        Some(ref prefix) => match col_name {
                            Some(name) => format!("{}[, \"{}\"]", prefix, name),
                            None => format!("{}[, {}]", prefix, i + 1),
                        },
                    };

                    let matrix_column = RFunction::from("[")
                        .add(object)
                        .param("i", R_MissingArg)
                        .param("j", (i + 1) as i32)
                        .call()?;

                    Self::extract_columns(*matrix_column, Some(name), row_count, columns)?;
                }
            }
        } else {
            r_assert_length(object, row_count)?;

            let data = {
                if r_is_simple_vector(object) {
                    harp::vector::format(object)
                } else {
                    let formatted = RFunction::from("format").add(object).call()?;
                    r_assert_type(*formatted, &[STRSXP])?;
                    r_assert_length(*formatted, row_count)?;
                    harp::vector::format(*formatted)
                }
            };

            columns.push(DataColumn {
                name: prefix.unwrap(),

                // TODO: String here is a placeholder
                column_type: String::from("String"),
                data,
            });
        }

        Ok(())
    }

    pub fn from_object(id: String, title: String, object: RObject) -> Result<Self, anyhow::Error> {
        r_lock! {
            let row_count = {
                if r_is_data_frame(*object) {
                    let row_names = Rf_getAttrib(*object, R_RowNamesSymbol);
                    r_xlength(row_names)
                } else if r_is_matrix(*object) {
                    let dim = Rf_getAttrib(*object, R_DimSymbol);
                    INTEGER_ELT(dim, 0) as isize
                } else {
                    bail!("data viewer only handles data frames and matrices")
                }
            };

            let mut columns = vec![];
            Self::extract_columns(*object, None, row_count, &mut columns)?;

            Ok(Self {
                id: id.clone(),
                title: title.clone(),
                columns: columns,
                row_count: row_count
            })
        }
    }
}

impl RDataViewer {
    pub fn start(title: String, data: RObject) {
        let id = Uuid::new_v4().to_string();
        spawn!(format!("ark-data-viewer-{}-{}", title, id), move || {
            let comm = CommSocket::new(
                CommInitiator::BackEnd,
                id.clone(),
                String::from("positron.dataViewer"),
            );
            let viewer = Self {
                id,
                title: title.clone(),
                data,
                comm,
            };
            viewer.execution_thread();
        });
    }

    pub fn execution_thread(self) {
        // This is a simplistic version where all the data is converted as once to
        // a message that is included in initial event of the comm.
        let json = match DataSet::from_object(self.id.clone(), self.title.clone(), self.data) {
            Ok(data_set) => serde_json::to_value(data_set).unwrap(),
            Err(error) => {
                log::error!("Error while viewing object '{}': {}", self.title, error);
                return;
            },
        };

        let comm_manager_tx = comm_manager_tx();
        let event = CommEvent::Opened(self.comm.clone(), json);
        comm_manager_tx.send(event).unwrap();
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_view_data_frame(x: SEXP, title: SEXP) -> SEXP {
    let title = match String::try_from(RObject::view(title)) {
        Ok(s) => s,
        Err(_) => String::from(""),
    };
    RDataViewer::start(title, RObject::from(x));

    R_NilValue
}
