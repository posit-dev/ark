//
// convert_to_code.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//
//

use amalthea::comm::data_explorer_comm::CodeSyntaxName;
use amalthea::comm::data_explorer_comm::ColumnDisplayType;
use amalthea::comm::data_explorer_comm::ConvertToCodeParams;
use amalthea::comm::data_explorer_comm::ConvertedCode;
use amalthea::comm::data_explorer_comm::FilterComparisonOp;
use amalthea::comm::data_explorer_comm::RowFilter;
use amalthea::comm::data_explorer_comm::RowFilterParams;
use amalthea::comm::data_explorer_comm::RowFilterType;
use amalthea::comm::data_explorer_comm::TextSearchType;

/// Convert the current data explorer view to executable code
///
/// Takes filters, sort keys, and other parameters and generates code that
/// can reproduce the current data view.
///
/// # Arguments
///
/// * `params` - Parameters for the code conversion including filters and sort keys
/// * `object_name` - Optional name of the data object in the R environment
///
/// # Returns
///
/// A `ConvertedCode` containing lines of code implementing the filters and sort keys
pub fn convert_to_code(params: ConvertToCodeParams, object_name: Option<&str>) -> ConvertedCode {
    // Create a library statement for dplyr
    let library_statement = "library(dplyr)".to_string();

    // Use a default placeholder if no object name is provided
    let object_ref = match object_name {
        Some(name) => name.to_string(),
        None => "dat".to_string(), // Default placeholder if no object name
    };

    // Start with the object reference
    let mut pipe_parts = vec![object_ref.clone()];

    // Add filter operations if there are any row filters
    if !params.row_filters.is_empty() {
        let filter_expressions = build_filter_expressions(&params.row_filters);
        if !filter_expressions.is_empty() {
            pipe_parts.push(format!(
                "filter(\n    {}\n  )",
                filter_expressions.join(",\n    ")
            ));
        }
    }

    // Always add the slice operation for now
    pipe_parts.push("slice(1:3)".to_string());

    // Join the parts with the pipe operator
    let pipe_expression = pipe_parts.join(" |>\n  ");

    // Combine the code lines
    ConvertedCode {
        converted_code: vec![library_statement, "".to_string(), pipe_expression],
    }
}

/// Builds filter expressions for dplyr from row filters
fn build_filter_expressions(row_filters: &[RowFilter]) -> Vec<String> {
    let mut expressions = Vec::new();

    for filter in row_filters {
        if let Some(expr) = row_filter_to_dplyr(filter) {
            expressions.push(expr);
        }
    }

    expressions
}

/// Formats a value for use in R code based on the column type
fn format_value_for_r(display_type: &ColumnDisplayType, value: &str) -> String {
    match display_type {
        // For strings, wrap in quotes
        ColumnDisplayType::String => quote_string(value),

        // For date and datetime types, wrap in quotes
        ColumnDisplayType::Date |
        ColumnDisplayType::Datetime |
        ColumnDisplayType::Time |
        ColumnDisplayType::Interval => quote_string(value),

        // For booleans, return as R logical constants
        ColumnDisplayType::Boolean => {
            if value.to_lowercase() == "true" {
                "TRUE".to_string()
            } else if value.to_lowercase() == "false" {
                "FALSE".to_string()
            } else {
                // If it's not clearly true/false, keep as is
                value.to_string()
            }
        },

        // For numbers, no quotes needed
        ColumnDisplayType::Number => value.to_string(),

        // For any other type, default to quoting
        _ => quote_string(value),
    }
}

/// Converts a single row filter to a dplyr filter expression
fn row_filter_to_dplyr(filter: &RowFilter) -> Option<String> {
    let column_name = &filter.column_schema.column_name;

    match filter.filter_type {
        RowFilterType::Compare => {
            if let Some(RowFilterParams::Comparison(comparison)) = &filter.params {
                let op = match comparison.op {
                    FilterComparisonOp::Eq => "==",
                    FilterComparisonOp::NotEq => "!=",
                    FilterComparisonOp::Lt => "<",
                    FilterComparisonOp::LtEq => "<=",
                    FilterComparisonOp::Gt => ">",
                    FilterComparisonOp::GtEq => ">=",
                };

                // Format the value based on the column's data type
                let value =
                    format_value_for_r(&filter.column_schema.type_display, &comparison.value);
                Some(format!("{} {} {}", column_name, op, value))
            } else {
                None
            }
        },
        RowFilterType::Between => {
            if let Some(RowFilterParams::Between(between)) = &filter.params {
                // Format values based on column type
                let left =
                    format_value_for_r(&filter.column_schema.type_display, &between.left_value);
                let right =
                    format_value_for_r(&filter.column_schema.type_display, &between.right_value);
                Some(format!(
                    "{} >= {} & {} <= {}",
                    column_name, left, column_name, right
                ))
            } else {
                None
            }
        },
        RowFilterType::NotBetween => {
            if let Some(RowFilterParams::Between(between)) = &filter.params {
                // Format values based on column type
                let left =
                    format_value_for_r(&filter.column_schema.type_display, &between.left_value);
                let right =
                    format_value_for_r(&filter.column_schema.type_display, &between.right_value);
                Some(format!(
                    "{} < {} | {} > {}",
                    column_name, left, column_name, right
                ))
            } else {
                None
            }
        },
        RowFilterType::IsNull => Some(format!("is.na({})", column_name)),
        RowFilterType::NotNull => Some(format!("!is.na({})", column_name)),
        RowFilterType::IsTrue => Some(format!("{} == TRUE", column_name)),
        RowFilterType::IsFalse => Some(format!("{} == FALSE", column_name)),
        RowFilterType::IsEmpty => Some(format!("{} == \"\"", column_name)),
        RowFilterType::NotEmpty => Some(format!("{} != \"\"", column_name)),
        RowFilterType::Search => {
            if let Some(RowFilterParams::TextSearch(search)) = &filter.params {
                match search.search_type {
                    TextSearchType::Contains => Some(format!(
                        "grepl({}, {}, fixed = TRUE)",
                        quote_string(&search.term),
                        column_name
                    )),
                    TextSearchType::NotContains => Some(format!(
                        "!grepl({}, {}, fixed = TRUE)",
                        quote_string(&search.term),
                        column_name
                    )),
                    TextSearchType::StartsWith => Some(format!(
                        "grepl({}, {}, fixed = TRUE)",
                        quote_string(&format!("^{}", escape_regex(&search.term))),
                        column_name
                    )),
                    TextSearchType::EndsWith => Some(format!(
                        "grepl({}, {}, fixed = TRUE)",
                        quote_string(&format!("{}$", escape_regex(&search.term))),
                        column_name
                    )),
                    TextSearchType::RegexMatch => Some(format!(
                        "grepl({}, {})",
                        quote_string(&search.term),
                        column_name
                    )),
                }
            } else {
                None
            }
        },
        RowFilterType::SetMembership => {
            if let Some(RowFilterParams::SetMembership(set)) = &filter.params {
                let values = set
                    .values
                    .iter()
                    .map(|v| quote_string(&v))
                    .collect::<Vec<_>>()
                    .join(", ");

                if set.inclusive {
                    Some(format!("{} %in% c({})", column_name, values))
                } else {
                    Some(format!("!({} %in% c({}))", column_name, values))
                }
            } else {
                None
            }
        },
    }
}

/// Properly quotes a string for R code
fn quote_string(s: &str) -> String {
    format!("\"{}\"", s.replace("\"", "\\\""))
}

/// Escapes special characters in regex patterns
fn escape_regex(s: &str) -> String {
    s.replace(".", "\\.")
        .replace("*", "\\*")
        .replace("+", "\\+")
        .replace("?", "\\?")
        .replace("[", "\\[")
        .replace("]", "\\]")
        .replace("(", "\\(")
        .replace(")", "\\)")
        .replace("{", "\\{")
        .replace("}", "\\}")
        .replace("^", "\\^")
        .replace("$", "\\$")
        .replace("|", "\\|")
        .replace("\\", "\\\\")
}

/// Suggest a code syntax based on available options
///
/// Currently always returns "dplyr" as the preferred syntax
///
/// # Returns
///
/// A `CodeSyntaxName` with the suggested syntax
pub fn suggest_code_syntax() -> CodeSyntaxName {
    CodeSyntaxName {
        code_syntax_name: "dplyr".into(),
    }
}
