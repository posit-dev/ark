//
// export_selection.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use amalthea::comm::data_explorer_comm::DataSelection;
use amalthea::comm::data_explorer_comm::DataSelectionCellRange;
use amalthea::comm::data_explorer_comm::DataSelectionIndices;
use amalthea::comm::data_explorer_comm::DataSelectionKind;
use amalthea::comm::data_explorer_comm::DataSelectionRange;
use amalthea::comm::data_explorer_comm::DataSelectionSingleCell;
use amalthea::comm::data_explorer_comm::ExportFormat;
use amalthea::comm::data_explorer_comm::Selection;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use libr::SEXP;

use crate::modules::ARK_ENVS;

// Returns the data frame exported in the requested format as a string
//
// Arguments:
// - data: The data frame full data frame to export
// - view_indices: The order of rows, and maybe filtered rows from the data frame to be selected.
//   Must be applied before the selection rules if selection affects rows.
// - selection: The selected region of the data frame
// - format: The format to export the data frame to (csv, tsv and html) are currently supported.
pub fn export_selection(
    data: SEXP,
    view_indices: Option<Vec<i32>>,
    selection: DataSelection,
    format: ExportFormat,
) -> anyhow::Result<String> {
    let region = get_selection(data, view_indices, selection.clone())?;
    let format_string = match format {
        ExportFormat::Csv => "csv",
        ExportFormat::Tsv => "tsv",
        ExportFormat::Html => "html",
    };
    let include_header = match selection.kind {
        DataSelectionKind::SingleCell => false,
        DataSelectionKind::CellRange => true,
        DataSelectionKind::RowRange => true,
        DataSelectionKind::ColumnRange => true,
        DataSelectionKind::ColumnIndices => true,
        DataSelectionKind::RowIndices => true,
    };
    Ok(RFunction::from("export_selection")
        .param("x", region)
        .param("format", format_string)
        .param("include_header", include_header)
        .call_in(ARK_ENVS.positron_ns)?
        .try_into()?)
}

fn get_selection(
    data: SEXP,
    view_indices: Option<Vec<i32>>,
    selection: DataSelection,
) -> anyhow::Result<SEXP> {
    let (i, j) = match selection.kind {
        DataSelectionKind::SingleCell => match selection.selection {
            Selection::SingleCell(DataSelectionSingleCell {
                row_index,
                column_index,
            }) => (Some(vec![row_index]), Some(vec![column_index])),
            _ => panic!("Invalid selection kind"),
        },
        DataSelectionKind::CellRange => match selection.selection {
            Selection::CellRange(DataSelectionCellRange {
                first_row_index,
                last_row_index,
                first_column_index,
                last_column_index,
            }) => (
                Some((first_row_index..=last_row_index).collect()),
                Some((first_column_index..=last_column_index).collect()),
            ),
            _ => panic!("Invalid selection kind"),
        },
        DataSelectionKind::RowRange => match selection.selection {
            Selection::IndexRange(DataSelectionRange {
                first_index,
                last_index,
            }) => (Some((first_index..=last_index).collect()), None),
            _ => panic!("Invalid selection kind"),
        },
        DataSelectionKind::ColumnRange => match selection.selection {
            Selection::IndexRange(DataSelectionRange {
                first_index,
                last_index,
            }) => (None, Some((first_index..=last_index).collect())),
            _ => panic!("Invalid selection kind"),
        },
        DataSelectionKind::ColumnIndices => match selection.selection {
            Selection::Indices(DataSelectionIndices { indices }) => (None, Some(indices)),
            _ => panic!("Invalid selection kind"),
        },
        DataSelectionKind::RowIndices => match selection.selection {
            Selection::Indices(DataSelectionIndices { indices }) => (Some(indices), None),
            _ => panic!("Invalid selection kind"),
        },
    };

    subset_with_view_indices(data, view_indices, i, j)
}

// This is responsible for converting 0-based indexes to 1-based indexes.
// Except for view_indices that are already 1-based.
fn subset_with_view_indices(
    x: SEXP,
    view_indices: Option<Vec<i32>>,
    i: Option<Vec<i64>>,
    j: Option<Vec<i64>>,
) -> anyhow::Result<SEXP> {
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
    r_table_subset(x, i, j)
}

fn r_table_subset(x: SEXP, i: Option<Vec<i64>>, j: Option<Vec<i64>>) -> anyhow::Result<SEXP> {
    let mut call = RFunction::from(".ps.table_subset");
    call.param("x", x);
    if let Some(i) = i {
        call.param("i", i);
    }
    if let Some(j) = j {
        call.param("j", j);
    }

    Ok(call.call_in(ARK_ENVS.positron_ns)?.sexp)
}

#[cfg(test)]
mod tests {
    use amalthea::comm::data_explorer_comm::DataSelection;
    use amalthea::comm::data_explorer_comm::DataSelectionKind;
    use amalthea::comm::data_explorer_comm::DataSelectionSingleCell;
    use amalthea::comm::data_explorer_comm::ExportFormat;
    use amalthea::comm::data_explorer_comm::Selection;
    use harp::environment::R_ENVS;
    use harp::eval::r_parse_eval0;
    use harp::object::RObject;

    use super::*;
    use crate::test::r_test;

    fn export_selection_helper(data: RObject, selection: DataSelection) -> String {
        let format = ExportFormat::Csv;
        export_selection(data.sexp, None, selection, format).unwrap()
    }

    #[test]
    fn test_single_cell_selection() {
        r_test(|| {
            let data = r_parse_eval0(
                "data.frame(a = 1:3, b = c(4,5,NA), c = letters[1:3])",
                R_ENVS.global,
            )
            .unwrap();

            let single_cell_selection = |i, j| DataSelection {
                kind: DataSelectionKind::SingleCell,
                selection: Selection::SingleCell(DataSelectionSingleCell {
                    row_index: i,
                    column_index: j,
                }),
            };

            // Basic test
            assert_eq!(
                export_selection_helper(data.clone(), single_cell_selection(1, 0)),
                "2".to_string()
            );

            // Strings are copied unquoted
            assert_eq!(
                export_selection_helper(data.clone(), single_cell_selection(0, 2)),
                "a".to_string()
            );

            // NA's are copied as empty strings
            assert_eq!(
                export_selection_helper(data.clone(), single_cell_selection(2, 1)),
                "".to_string()
            );
        });
    }
}
