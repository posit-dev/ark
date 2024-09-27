//
// export_selection.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use amalthea::comm::data_explorer_comm::DataSelectionCellRange;
use amalthea::comm::data_explorer_comm::DataSelectionIndices;
use amalthea::comm::data_explorer_comm::DataSelectionRange;
use amalthea::comm::data_explorer_comm::DataSelectionSingleCell;
use amalthea::comm::data_explorer_comm::ExportFormat;
use amalthea::comm::data_explorer_comm::Selection;
use amalthea::comm::data_explorer_comm::TableSelection;
use amalthea::comm::data_explorer_comm::TableSelectionKind;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use libr::SEXP;

use crate::data_explorer::utils::tbl_subset_with_view_indices;
use crate::modules::ARK_ENVS;

// Returns the data frame exported in the requested format as a string
//
// Arguments:
// - data: The data frame full data frame to export
// - view_indices: The order of rows, and maybe filtered rows from the data frame to be selected.
//   Must be applied before the selection rules if selection affects rows.
// - selection: The selected region of the data frame
// - format: The format to export the data frame to (csv, tsv and html are currently supported).
pub fn export_selection(
    data: SEXP,
    view_indices: &Option<Vec<i32>>,
    selection: TableSelection,
    format: ExportFormat,
) -> anyhow::Result<String> {
    let region = get_selection(data, view_indices, selection.clone())?;
    let format_string = match format {
        ExportFormat::Csv => "csv",
        ExportFormat::Tsv => "tsv",
        ExportFormat::Html => "html",
    };
    let include_header = match selection.kind {
        TableSelectionKind::SingleCell => false,
        TableSelectionKind::CellRange => true,
        TableSelectionKind::RowRange => true,
        TableSelectionKind::ColumnRange => true,
        TableSelectionKind::ColumnIndices => true,
        TableSelectionKind::RowIndices => true,
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
    view_indices: &Option<Vec<i32>>,
    selection: TableSelection,
) -> anyhow::Result<RObject> {
    let (i, j) = match selection.kind {
        TableSelectionKind::SingleCell => match selection.selection {
            Selection::SingleCell(DataSelectionSingleCell {
                row_index,
                column_index,
            }) => (Some(vec![row_index]), Some(vec![column_index])),
            _ => panic!("Invalid selection kind"),
        },
        TableSelectionKind::CellRange => match selection.selection {
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
        TableSelectionKind::RowRange => match selection.selection {
            Selection::IndexRange(DataSelectionRange {
                first_index,
                last_index,
            }) => (Some((first_index..=last_index).collect()), None),
            _ => panic!("Invalid selection kind"),
        },
        TableSelectionKind::ColumnRange => match selection.selection {
            Selection::IndexRange(DataSelectionRange {
                first_index,
                last_index,
            }) => (None, Some((first_index..=last_index).collect())),
            _ => panic!("Invalid selection kind"),
        },
        TableSelectionKind::ColumnIndices => match selection.selection {
            Selection::Indices(DataSelectionIndices { indices }) => (None, Some(indices)),
            _ => panic!("Invalid selection kind"),
        },
        TableSelectionKind::RowIndices => match selection.selection {
            Selection::Indices(DataSelectionIndices { indices }) => (Some(indices), None),
            _ => panic!("Invalid selection kind"),
        },
    };

    tbl_subset_with_view_indices(data, view_indices, i, j)
}

#[cfg(test)]
mod tests {
    use amalthea::comm::data_explorer_comm::DataSelectionSingleCell;
    use amalthea::comm::data_explorer_comm::ExportFormat;
    use amalthea::comm::data_explorer_comm::Selection;
    use harp::object::RObject;

    use super::*;
    use crate::fixtures::r_test;

    fn export_selection_helper(data: RObject, selection: TableSelection) -> String {
        export_selection_helper_with_format(data, selection, ExportFormat::Csv)
    }

    fn export_selection_helper_with_format(
        data: RObject,
        selection: TableSelection,
        format: ExportFormat,
    ) -> String {
        export_selection(data.sexp, &None, selection, format).unwrap()
    }

    fn export_selection_helper_with_view_indices(
        data: RObject,
        view_indices: Vec<i32>,
        selection: TableSelection,
    ) -> String {
        export_selection(data.sexp, &Some(view_indices), selection, ExportFormat::Csv).unwrap()
    }

    fn small_test_data() -> RObject {
        harp::parse_eval_global("data.frame(a = 1:3, b = c(4,5,NA), c = letters[1:3])").unwrap()
    }

    fn has_knitr() -> bool {
        let res: Option<bool> =
            harp::parse_eval0(r#".ps.is_installed("knitr")"#, ARK_ENVS.positron_ns)
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

            let single_cell_selection = |i, j| TableSelection {
                kind: TableSelectionKind::SingleCell,
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

            let cell_range_selection = |i1, i2, j1, j2| TableSelection {
                kind: TableSelectionKind::CellRange,
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

            let row_range_selection = |i1, i2| TableSelection {
                kind: TableSelectionKind::RowRange,
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

            let col_range_selection = |j1, j2| TableSelection {
                kind: TableSelectionKind::ColumnRange,
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

            let row_indices_selection = |indices| TableSelection {
                kind: TableSelectionKind::RowIndices,
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

            let col_indices_selection = |indices| TableSelection {
                kind: TableSelectionKind::ColumnIndices,
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

    #[test]
    fn test_view_indices() {
        r_test(|| {
            let data = small_test_data();

            let single_cell_selection = |i, j| TableSelection {
                kind: TableSelectionKind::SingleCell,
                selection: Selection::SingleCell(DataSelectionSingleCell {
                    row_index: i,
                    column_index: j,
                }),
            };

            // view indices imply a different ordering of the data
            // note: view_indices are 1 based!
            assert_eq!(
                export_selection_helper_with_view_indices(
                    data.clone(),
                    vec![3, 2, 1],
                    single_cell_selection(0, 1)
                ),
                "".to_string()
            );

            // view indices imply a different subset of the data
            assert_eq!(
                export_selection_helper_with_view_indices(
                    data.clone(),
                    vec![2],
                    single_cell_selection(0, 1)
                ),
                "5".to_string()
            );
        })
    }
}
