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
        export_selection_helper_with_format(data, selection, ExportFormat::Csv)
    }

    fn export_selection_helper_with_format(
        data: RObject,
        selection: DataSelection,
        format: ExportFormat,
    ) -> String {
        export_selection(data.sexp, None, selection, format).unwrap()
    }

    fn small_test_data() -> RObject {
        r_parse_eval0(
            "data.frame(a = 1:3, b = c(4,5,NA), c = letters[1:3])",
            R_ENVS.global,
        )
        .unwrap()
    }

    fn has_knitr() -> bool {
        let res: Option<bool> = r_parse_eval0(r#".ps.is_installed("knitr")"#, ARK_ENVS.positron_ns)
            .unwrap()
            .try_into()
            .unwrap();
        match res {
            Some(res) => res,
            None => false,
        }
    }

    #[test]
    fn test_single_cell_selection() {
        r_test(|| {
            let data = small_test_data();

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

            if has_knitr() {
                // HTML format
                assert!(export_selection_helper_with_format(
                    data.clone(),
                    single_cell_selection(0, 1),
                    ExportFormat::Html
                )
                .contains("<table>"));

                // HTML format, NA's handling
                assert!(export_selection_helper_with_format(
                    data.clone(),
                    single_cell_selection(2, 1),
                    ExportFormat::Html
                )
                .contains(r#"<td style="text-align:right;">  </td>"#)); // NA's are formatted as empty strings
            }
        });
    }

    #[test]
    fn test_cell_range_selection() {
        r_test(|| {
            let data = small_test_data();

            let cell_range_selection = |i1, i2, j1, j2| DataSelection {
                kind: DataSelectionKind::CellRange,
                selection: Selection::CellRange(DataSelectionCellRange {
                    first_row_index: i1,
                    last_row_index: i2,
                    first_column_index: j1,
                    last_column_index: j2,
                }),
            };

            // Basic test
            assert_eq!(
                export_selection_helper(data.clone(), cell_range_selection(0, 1, 0, 1)),
                "a,b\n1,4\n2,5".to_string()
            );

            // Strings are copied unquoted
            assert_eq!(
                export_selection_helper(data.clone(), cell_range_selection(0, 1, 1, 2)),
                "b,c\n4,a\n5,b".to_string()
            );

            // NA's are copied as empty strings
            assert_eq!(
                export_selection_helper(data.clone(), cell_range_selection(1, 2, 1, 2)),
                "b,c\n5,b\n,c".to_string()
            );

            if has_knitr() {
                // test HTML format
                assert!(export_selection_helper_with_format(
                    data.clone(),
                    cell_range_selection(1, 2, 1, 2),
                    ExportFormat::Html
                )
                .contains("<thead>")); // test that contais a table header
            }
        });
    }

    #[test]
    fn test_row_range_selection() {
        r_test(|| {
            let data = small_test_data();

            let row_range_selection = |i1, i2| DataSelection {
                kind: DataSelectionKind::RowRange,
                selection: Selection::IndexRange(DataSelectionRange {
                    first_index: i1,
                    last_index: i2,
                }),
            };

            // Basic test
            assert_eq!(
                export_selection_helper(data.clone(), row_range_selection(0, 1)),
                "a,b,c\n1,4,a\n2,5,b".to_string()
            );

            // Strings are copied unquoted
            assert_eq!(
                export_selection_helper(data.clone(), row_range_selection(1, 2)),
                "a,b,c\n2,5,b\n3,,c".to_string()
            );

            // NA's are copied as empty strings
            assert_eq!(
                export_selection_helper(data.clone(), row_range_selection(2, 2)),
                "a,b,c\n3,,c".to_string()
            );
        });
    }

    #[test]
    fn test_col_range_selection() {
        r_test(|| {
            let data = small_test_data();

            let col_range_selection = |j1, j2| DataSelection {
                kind: DataSelectionKind::ColumnRange,
                selection: Selection::IndexRange(DataSelectionRange {
                    first_index: j1,
                    last_index: j2,
                }),
            };

            // Basic test
            assert_eq!(
                export_selection_helper(data.clone(), col_range_selection(0, 1)),
                "a,b\n1,4\n2,5\n3,".to_string()
            );

            // Strings are copied unquoted
            assert_eq!(
                export_selection_helper(data.clone(), col_range_selection(1, 2)),
                "b,c\n4,a\n5,b\n,c".to_string()
            );

            // NA's are copied as empty strings
            assert_eq!(
                export_selection_helper(data.clone(), col_range_selection(2, 2)),
                "c\na\nb\nc".to_string()
            );
        });
    }

    #[test]
    fn test_row_indices_selection() {
        r_test(|| {
            let data = small_test_data();

            let row_indices_selection = |indices| DataSelection {
                kind: DataSelectionKind::RowIndices,
                selection: Selection::Indices(DataSelectionIndices { indices }),
            };

            // Basic test
            assert_eq!(
                export_selection_helper(data.clone(), row_indices_selection(vec![0, 2])),
                "a,b,c\n1,4,a\n3,,c".to_string()
            );

            // Strings are copied unquoted
            assert_eq!(
                export_selection_helper(data.clone(), row_indices_selection(vec![1, 2])),
                "a,b,c\n2,5,b\n3,,c".to_string()
            );

            // NA's are copied as empty strings
            assert_eq!(
                export_selection_helper(data.clone(), row_indices_selection(vec![2])),
                "a,b,c\n3,,c".to_string()
            );
        });
    }

    #[test]
    fn test_col_indices_selection() {
        r_test(|| {
            let data = small_test_data();

            let col_indices_selection = |indices| DataSelection {
                kind: DataSelectionKind::ColumnIndices,
                selection: Selection::Indices(DataSelectionIndices { indices }),
            };

            // Basic test
            assert_eq!(
                export_selection_helper(data.clone(), col_indices_selection(vec![0, 2])),
                "a,c\n1,a\n2,b\n3,c".to_string()
            );

            // Strings are copied unquoted
            assert_eq!(
                export_selection_helper(data.clone(), col_indices_selection(vec![1, 2])),
                "b,c\n4,a\n5,b\n,c".to_string()
            );

            // NA's are copied as empty strings
            assert_eq!(
                export_selection_helper(data.clone(), col_indices_selection(vec![2])),
                "c\na\nb\nc".to_string()
            );
        });
    }
}
