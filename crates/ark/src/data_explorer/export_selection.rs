//
// export_selection.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use amalthea::comm::data_explorer_comm::DataSelectionCellIndices;
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
        TableSelectionKind::CellIndices => true,
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
        TableSelectionKind::CellIndices => match selection.selection {
            Selection::CellIndices(DataSelectionCellIndices {
                row_indices,
                column_indices,
            }) => (Some(row_indices), Some(column_indices)),
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
    use amalthea::comm::data_explorer_comm::DataSelectionCellIndices;
    use amalthea::comm::data_explorer_comm::DataSelectionCellRange;
    use amalthea::comm::data_explorer_comm::DataSelectionIndices;
    use amalthea::comm::data_explorer_comm::DataSelectionRange;
    use amalthea::comm::data_explorer_comm::DataSelectionSingleCell;
    use amalthea::comm::data_explorer_comm::ExportFormat;
    use amalthea::comm::data_explorer_comm::Selection;
    use harp::object::RObject;

    use super::*;
    use crate::r_task;

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

    /// Test data that's easier to verify and understand in assertions
    /// Creates:
    /// row | col_0 | col_1 | col_2
    ///  0  |  10   |  20   |  'A'
    ///  1  |  11   |  21   |  'B'  
    ///  2  |  12   |  22   |  'C'
    ///  3  |  13   |  23   |  'D'
    fn predictable_test_data() -> RObject {
        harp::parse_eval_global("data.frame(col_0 = 10:13, col_1 = 20:23, col_2 = LETTERS[1:4])")
            .unwrap()
    }

    // Helper functions to create different selection types
    fn single_cell_selection(row_index: i64, column_index: i64) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::SingleCell,
            selection: Selection::SingleCell(DataSelectionSingleCell {
                row_index,
                column_index,
            }),
        }
    }

    fn cell_range_selection(
        first_row_index: i64,
        last_row_index: i64,
        first_column_index: i64,
        last_column_index: i64,
    ) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::CellRange,
            selection: Selection::CellRange(DataSelectionCellRange {
                first_row_index,
                last_row_index,
                first_column_index,
                last_column_index,
            }),
        }
    }

    fn cell_indices_selection(row_indices: Vec<i64>, column_indices: Vec<i64>) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::CellIndices,
            selection: Selection::CellIndices(DataSelectionCellIndices {
                row_indices,
                column_indices,
            }),
        }
    }

    fn row_range_selection(first_index: i64, last_index: i64) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::RowRange,
            selection: Selection::IndexRange(DataSelectionRange {
                first_index,
                last_index,
            }),
        }
    }

    fn column_range_selection(first_index: i64, last_index: i64) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::ColumnRange,
            selection: Selection::IndexRange(DataSelectionRange {
                first_index,
                last_index,
            }),
        }
    }

    fn row_indices_selection(indices: Vec<i64>) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::RowIndices,
            selection: Selection::Indices(DataSelectionIndices { indices }),
        }
    }

    fn column_indices_selection(indices: Vec<i64>) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::ColumnIndices,
            selection: Selection::Indices(DataSelectionIndices { indices }),
        }
    }

    /// Check if knitr is available for HTML export tests.
    /// HTML export requires knitr::kable(), so these tests are conditional.
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

    /// Test a selection with all supported formats (CSV, TSV, and HTML if knitr is available)
    fn test_selection_all_formats(data: &RObject, selection: TableSelection, expected_csv: &str) {
        // Test CSV format
        assert_eq!(
            export_selection_helper(data.clone(), selection.clone()),
            expected_csv
        );

        // Test TSV format (should be same as CSV but with tab separators)
        let expected_tsv = expected_csv.replace(',', "\t");
        assert_eq!(
            export_selection_helper_with_format(data.clone(), selection.clone(), ExportFormat::Tsv),
            expected_tsv
        );

        // Test HTML format if knitr is available
        if has_knitr() {
            let html_result =
                export_selection_helper_with_format(data.clone(), selection, ExportFormat::Html);

            // HTML should contain table elements for multi-cell selections
            if expected_csv.contains('\n') && expected_csv.lines().count() > 1 {
                assert!(
                    html_result.contains("<table"),
                    "HTML should contain table for multi-row selection"
                );
                assert!(
                    html_result.contains("<thead"),
                    "HTML should contain table header"
                );
            } else {
                // Single cell selections just return the value
                assert!(
                    !html_result.contains("<table"),
                    "Single cell HTML should not contain table"
                );
            }
        }
    }

    #[test]
    fn test_na_value_handling() {
        r_task(|| {
            let data = small_test_data(); // data.frame(a = 1:3, b = c(4,5,NA), c = letters[1:3])

            // Test NA handling in single cell - NA should export as empty string
            assert_eq!(
                export_selection_helper(data.clone(), single_cell_selection(2, 1)),
                "".to_string() // NA exported as empty string
            );

            // Test NA handling in multi-cell export - NA should appear as empty in CSV
            test_selection_all_formats(
                &data,
                cell_range_selection(1, 2, 1, 2),
                "b,c\n5,b\n,c", // NA in row 2, col 1 (b column) appears as empty
            );
        });
    }

    #[test]
    fn test_cell_indices_order_preservation() {
        r_task(|| {
            let data = predictable_test_data();

            // Test case 1: Single row, multiple columns
            test_selection_all_formats(
                &data,
                cell_indices_selection(vec![1], vec![0, 2]),
                "col_0,col_2\n11,B",
            );

            // Test case 2: Multiple rows, single column
            test_selection_all_formats(
                &data,
                cell_indices_selection(vec![0, 2], vec![1]),
                "col_1\n20\n22",
            );

            // Test case 3: Cartesian product - multiple rows × multiple columns
            test_selection_all_formats(
                &data,
                cell_indices_selection(vec![0, 1], vec![0, 2]),
                "col_0,col_2\n10,A\n11,B",
            );

            // Test case 4: Order preservation - non-increasing row indices
            test_selection_all_formats(
                &data,
                cell_indices_selection(vec![3, 0, 2], vec![0]),
                "col_0\n13\n10\n12",
            );

            // Test case 5: Order preservation - non-increasing column indices
            test_selection_all_formats(
                &data,
                cell_indices_selection(vec![1], vec![2, 0, 1]),
                "col_2,col_0,col_1\nB,11,21",
            );

            // Test case 6: Both rows and columns out of order
            test_selection_all_formats(
                &data,
                cell_indices_selection(vec![2, 0], vec![1, 2]),
                "col_1,col_2\n22,C\n20,A",
            );

            // Test case 7: Single cell (edge case)
            assert_eq!(
                export_selection_helper(data.clone(), cell_indices_selection(vec![1], vec![1])),
                "col_1\n21"
            );
        });
    }

    #[test]
    fn test_all_selection_types() {
        r_task(|| {
            let data = predictable_test_data();

            // Test single cell selection - exports just the cell value without headers
            test_selection_all_formats(
                &data,
                single_cell_selection(1, 2), // row 1, col 2 -> 'B' (no header for single cell)
                "B",
            );

            // Test cell range - rectangular selection with headers
            test_selection_all_formats(
                &data,
                cell_range_selection(1, 2, 0, 1), // rows 1-2, cols 0-1
                "col_0,col_1\n11,21\n12,22",
            );

            // Test row range - full rows from first to last index
            test_selection_all_formats(
                &data,
                row_range_selection(1, 2), // rows 1-2, all columns
                "col_0,col_1,col_2\n11,21,B\n12,22,C",
            );

            // Test column range - full columns from first to last index
            test_selection_all_formats(
                &data,
                column_range_selection(0, 1), // all rows, cols 0-1
                "col_0,col_1\n10,20\n11,21\n12,22\n13,23",
            );

            // Test row indices - specific rows in given order (non-contiguous)
            test_selection_all_formats(
                &data,
                row_indices_selection(vec![0, 3, 1]), // rows 0, 3, 1 in that order
                "col_0,col_1,col_2\n10,20,A\n13,23,D\n11,21,B",
            );

            // Test column indices - specific columns in given order (non-contiguous)
            test_selection_all_formats(
                &data,
                column_indices_selection(vec![2, 0]), // cols 2, 0 in that order
                "col_2,col_0\nA,10\nB,11\nC,12\nD,13",
            );
        });
    }

    #[test]
    fn test_view_indices_filtering() {
        r_task(|| {
            let data = small_test_data();

            // View indices allow reordering and filtering of rows before selection
            // Note: view_indices are 1-based in R!
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

    #[test]
    fn test_cross_platform_line_endings() {
        r_task(|| {
            let data = predictable_test_data();

            // Ensure consistent line ending behavior across Windows and Unix
            // R's write functions should produce \n regardless of platform
            let result =
                export_selection_helper(data.clone(), cell_indices_selection(vec![0, 1], vec![0]));

            // Should always use Unix line endings for consistency
            assert!(result.contains('\n'), "Should contain Unix line endings");
            assert!(
                !result.contains("\r\n"),
                "Should not contain Windows CRLF line endings"
            );

            // Verify exact content with no trailing newlines
            assert_eq!(result, "col_0\n10\n11");
        });
    }
}
