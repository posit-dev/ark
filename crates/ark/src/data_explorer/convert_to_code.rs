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

/// Sort key with resolved column name
#[derive(Clone, Debug)]
pub struct ResolvedSortKey {
    pub column_name: String,
    pub ascending: bool,
}

/// Base trait for handling row filter conversion to code
trait FilterHandler {
    fn convert_filter(&self, filter: &RowFilter) -> Option<String>;
}

/// Base trait for handling sort key conversion to code
trait SortHandler {
    fn convert_sorts(&self, sort_keys: &[ResolvedSortKey]) -> Option<String>;
}

/// Base trait for code converters that generate final code output
trait CodeConverter {
    fn build_code(&self, params: ConvertToCodeParams, object_name: Option<&str>, resolved_sort_keys: &[ResolvedSortKey]) -> ConvertedCode;
}

/// Helper for building pipe chains with library imports
struct PipeBuilder {
    table_name: String,
    operations: Vec<String>,
}

impl PipeBuilder {
    fn new(table_name: String) -> Self {
        Self {
            table_name,
            operations: Vec::new(),
        }
    }

    fn add_operation(&mut self, operation: String) {
        self.operations.push(operation);
    }

    fn build(self, library_imports: Vec<String>) -> ConvertedCode {
        let mut code_lines = library_imports;

        if !code_lines.is_empty() {
            code_lines.push("".to_string()); // Empty line after imports
        }

        // Build the pipe expression
        if self.operations.is_empty() {
            code_lines.push(self.table_name);
        } else {
            let mut pipe_parts = vec![self.table_name];
            pipe_parts.extend(self.operations);
            code_lines.push(pipe_parts.join(" |>\n  "));
        }

        ConvertedCode {
            converted_code: code_lines,
        }
    }
}

/// Dplyr-specific filter handler
struct DplyrFilterHandler;

impl FilterHandler for DplyrFilterHandler {
    fn convert_filter(&self, filter: &RowFilter) -> Option<String> {
        row_filter_to_dplyr(filter)
    }
}

impl DplyrFilterHandler {
    fn convert_filters(&self, filters: &[RowFilter]) -> Option<String> {
        if filters.is_empty() {
            return None;
        }

        let filter_expressions: Vec<String> = filters
            .iter()
            .filter_map(|filter| self.convert_filter(filter))
            .collect();

        if filter_expressions.is_empty() {
            None
        } else {
            Some(format!(
                "filter(\n    {}\n  )",
                filter_expressions.join(",\n    ")
            ))
        }
    }
}

/// Dplyr-specific sort handler
struct DplyrSortHandler;

impl SortHandler for DplyrSortHandler {
    fn convert_sorts(&self, sort_keys: &[ResolvedSortKey]) -> Option<String> {
        if sort_keys.is_empty() {
            return None;
        }

        let sort_expressions: Vec<String> = sort_keys
            .iter()
            .map(|sort_key| {
                if sort_key.ascending {
                    sort_key.column_name.clone()
                } else {
                    format!("desc({})", sort_key.column_name)
                }
            })
            .collect();

        Some(format!("arrange({})", sort_expressions.join(", ")))
    }
}

/// Dplyr-specific code converter
struct DplyrCodeConverter;

impl CodeConverter for DplyrCodeConverter {
    fn build_code(&self, params: ConvertToCodeParams, object_name: Option<&str>, resolved_sort_keys: &[ResolvedSortKey]) -> ConvertedCode {
        let table_name = object_name.unwrap_or("dat").to_string();
        let mut builder = PipeBuilder::new(table_name);

        let filter_handler = DplyrFilterHandler;
        let sort_handler = DplyrSortHandler;

        // Add filter operations
        if let Some(filter_op) = filter_handler.convert_filters(&params.row_filters) {
            builder.add_operation(filter_op);
        }

        // Add sort operations using resolved sort keys
        if let Some(sort_op) = sort_handler.convert_sorts(resolved_sort_keys) {
            builder.add_operation(sort_op);
        }

        builder.build(vec!["library(dplyr)".to_string()])
    }
}

/// Convert the current data explorer view to executable code
///
/// Takes filters, sort keys, and other parameters and generates code that
/// can reproduce the current data view.
///
/// # Arguments
///
/// * `params` - Parameters for the code conversion including filters and sort keys
/// * `object_name` - Optional name of the data object in the R environment
/// * `resolved_sort_keys` - Sort keys with resolved column names
///
/// # Returns
///
/// A `ConvertedCode` containing lines of code implementing the filters and sort keys
pub fn convert_to_code(params: ConvertToCodeParams, object_name: Option<&str>, resolved_sort_keys: &[ResolvedSortKey]) -> ConvertedCode {
    // For now, default to dplyr syntax
    // TODO: Use params.code_syntax_name to choose the appropriate converter
    let converter = DplyrCodeConverter;
    converter.build_code(params, object_name, resolved_sort_keys)
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

#[cfg(test)]
mod tests {
    use super::*;
    use amalthea::comm::data_explorer_comm::ColumnSchema;
    use amalthea::comm::data_explorer_comm::FilterBetween;
    use amalthea::comm::data_explorer_comm::FilterComparison;
    use amalthea::comm::data_explorer_comm::FilterTextSearch;
    use amalthea::comm::data_explorer_comm::RowFilterCondition;

    /// Helper to create a test ColumnSchema
    fn test_column_schema(name: &str, display_type: ColumnDisplayType) -> ColumnSchema {
        ColumnSchema {
            column_name: name.to_string(),
            column_label: None,
            column_index: 0,
            type_name: "test".to_string(),
            type_display: display_type,
            description: None,
            children: None,
            precision: None,
            scale: None,
            timezone: None,
            type_size: None,
        }
    }

    /// Helper to create a comparison filter
    fn comparison_filter(
        column_name: &str,
        op: FilterComparisonOp,
        value: &str,
        display_type: ColumnDisplayType,
    ) -> RowFilter {
        RowFilter {
            filter_id: "test".to_string(),
            column_schema: test_column_schema(column_name, display_type),
            filter_type: RowFilterType::Compare,
            condition: RowFilterCondition::And,
            params: Some(RowFilterParams::Comparison(FilterComparison {
                op,
                value: value.to_string(),
            })),
            is_valid: Some(true),
            error_message: None,
        }
    }

    /// Helper to create a between filter
    fn between_filter(
        column_name: &str,
        left: &str,
        right: &str,
        display_type: ColumnDisplayType,
    ) -> RowFilter {
        RowFilter {
            filter_id: "test".to_string(),
            column_schema: test_column_schema(column_name, display_type),
            filter_type: RowFilterType::Between,
            condition: RowFilterCondition::And,
            params: Some(RowFilterParams::Between(FilterBetween {
                left_value: left.to_string(),
                right_value: right.to_string(),
            })),
            is_valid: Some(true),
            error_message: None,
        }
    }

    /// Helper to create a text search filter
    fn text_search_filter(
        column_name: &str,
        term: &str,
        search_type: TextSearchType,
        case_sensitive: bool,
    ) -> RowFilter {
        RowFilter {
            filter_id: "test".to_string(),
            column_schema: test_column_schema(column_name, ColumnDisplayType::String),
            filter_type: RowFilterType::Search,
            condition: RowFilterCondition::And,
            params: Some(RowFilterParams::TextSearch(FilterTextSearch {
                search_type,
                term: term.to_string(),
                case_sensitive,
            })),
            is_valid: Some(true),
            error_message: None,
        }
    }

    #[test]
    fn test_pipe_builder_empty() {
        let builder = PipeBuilder::new("data".to_string());
        let result = builder.build(vec!["library(dplyr)".to_string()]);

        assert_eq!(result.converted_code, vec![
            "library(dplyr)".to_string(),
            "".to_string(),
            "data".to_string()
        ]);
    }

    #[test]
    fn test_pipe_builder_with_operations() {
        let mut builder = PipeBuilder::new("data".to_string());
        builder.add_operation("filter(x > 1)".to_string());
        builder.add_operation("arrange(y)".to_string());

        let result = builder.build(vec!["library(dplyr)".to_string()]);

        assert_eq!(result.converted_code, vec![
            "library(dplyr)".to_string(),
            "".to_string(),
            "data |>\n  filter(x > 1) |>\n  arrange(y)".to_string()
        ]);
    }

    #[test]
    fn test_resolved_sort_key_single() {
        let sort_handler = DplyrSortHandler;
        let sort_keys = vec![ResolvedSortKey {
            column_name: "price".to_string(),
            ascending: true,
        }];

        let result = sort_handler.convert_sorts(&sort_keys);
        assert_eq!(result, Some("arrange(price)".to_string()));
    }

    #[test]
    fn test_resolved_sort_key_descending() {
        let sort_handler = DplyrSortHandler;
        let sort_keys = vec![ResolvedSortKey {
            column_name: "sales".to_string(),
            ascending: false,
        }];

        let result = sort_handler.convert_sorts(&sort_keys);
        assert_eq!(result, Some("arrange(desc(sales))".to_string()));
    }

    #[test]
    fn test_resolved_sort_key_multiple() {
        let sort_handler = DplyrSortHandler;
        let sort_keys = vec![
            ResolvedSortKey {
                column_name: "sales".to_string(),
                ascending: false,
            },
            ResolvedSortKey {
                column_name: "region".to_string(),
                ascending: true,
            },
            ResolvedSortKey {
                column_name: "date".to_string(),
                ascending: false,
            },
        ];

        let result = sort_handler.convert_sorts(&sort_keys);
        assert_eq!(result, Some("arrange(desc(sales), region, desc(date))".to_string()));
    }

    #[test]
    fn test_filter_comparison_operators() {
        let filter_handler = DplyrFilterHandler;

        let test_cases = vec![
            (FilterComparisonOp::Eq, "=="),
            (FilterComparisonOp::NotEq, "!="),
            (FilterComparisonOp::Lt, "<"),
            (FilterComparisonOp::LtEq, "<="),
            (FilterComparisonOp::Gt, ">"),
            (FilterComparisonOp::GtEq, ">="),
        ];

        for (op, expected_op) in test_cases {
            let filter = comparison_filter("price", op, "100", ColumnDisplayType::Number);
            let result = filter_handler.convert_filter(&filter);
            assert_eq!(result, Some(format!("price {} 100", expected_op)));
        }
    }

    #[test]
    fn test_filter_string_values() {
        let filter_handler = DplyrFilterHandler;
        let filter = comparison_filter("category", FilterComparisonOp::Eq, "Electronics", ColumnDisplayType::String);

        let result = filter_handler.convert_filter(&filter);
        assert_eq!(result, Some("category == \"Electronics\"".to_string()));
    }

    #[test]
    fn test_filter_boolean_values() {
        let filter_handler = DplyrFilterHandler;

        let filter_true = comparison_filter("active", FilterComparisonOp::Eq, "true", ColumnDisplayType::Boolean);
        let result_true = filter_handler.convert_filter(&filter_true);
        assert_eq!(result_true, Some("active == TRUE".to_string()));

        let filter_false = comparison_filter("active", FilterComparisonOp::Eq, "false", ColumnDisplayType::Boolean);
        let result_false = filter_handler.convert_filter(&filter_false);
        assert_eq!(result_false, Some("active == FALSE".to_string()));
    }

    #[test]
    fn test_filter_between() {
        let filter_handler = DplyrFilterHandler;
        let filter = between_filter("price", "100", "500", ColumnDisplayType::Number);

        let result = filter_handler.convert_filter(&filter);
        assert_eq!(result, Some("price >= 100 & price <= 500".to_string()));
    }

    #[test]
    fn test_filter_text_search() {
        let filter_handler = DplyrFilterHandler;

        // Test contains
        let contains_filter = text_search_filter("name", "john", TextSearchType::Contains, true);
        let result = filter_handler.convert_filter(&contains_filter);
        assert_eq!(result, Some("grepl(\"john\", name, fixed = TRUE)".to_string()));

        // Test starts with
        let starts_filter = text_search_filter("name", "Mr", TextSearchType::StartsWith, true);
        let result = filter_handler.convert_filter(&starts_filter);
        assert_eq!(result, Some("grepl(\"^Mr\", name, fixed = TRUE)".to_string()));
    }

    #[test]
    fn test_multiple_filters() {
        let filter_handler = DplyrFilterHandler;
        let filters = vec![
            comparison_filter("price", FilterComparisonOp::Gt, "100", ColumnDisplayType::Number),
            comparison_filter("category", FilterComparisonOp::Eq, "Electronics", ColumnDisplayType::String),
        ];

        let result = filter_handler.convert_filters(&filters);
        assert_eq!(result, Some("filter(\n    price > 100,\n    category == \"Electronics\"\n  )".to_string()));
    }

    #[test]
    fn test_complete_conversion_filters_only() {
        let params = ConvertToCodeParams {
            column_filters: vec![],
            row_filters: vec![
                comparison_filter("price", FilterComparisonOp::Gt, "100", ColumnDisplayType::Number),
            ],
            sort_keys: vec![],
            code_syntax_name: amalthea::comm::data_explorer_comm::CodeSyntaxName {
                code_syntax_name: "dplyr".to_string(),
            },
        };

        let result = convert_to_code(params, Some("my_data"), &[]);

        assert_eq!(result.converted_code, vec![
            "library(dplyr)".to_string(),
            "".to_string(),
            "my_data |>\n  filter(\n    price > 100\n  )".to_string(),
        ]);
    }

    #[test]
    fn test_complete_conversion_sorts_only() {
        let params = ConvertToCodeParams {
            column_filters: vec![],
            row_filters: vec![],
            sort_keys: vec![], // Note: sort_keys in params are ignored, we use resolved_sort_keys
            code_syntax_name: amalthea::comm::data_explorer_comm::CodeSyntaxName {
                code_syntax_name: "dplyr".to_string(),
            },
        };

        let resolved_sorts = vec![ResolvedSortKey {
            column_name: "date".to_string(),
            ascending: false,
        }];

        let result = convert_to_code(params, Some("my_data"), &resolved_sorts);

        assert_eq!(result.converted_code, vec![
            "library(dplyr)".to_string(),
            "".to_string(),
            "my_data |>\n  arrange(desc(date))".to_string(),
        ]);
    }

    #[test]
    fn test_complete_conversion_filters_and_sorts() {
        let params = ConvertToCodeParams {
            column_filters: vec![],
            row_filters: vec![
                comparison_filter("price", FilterComparisonOp::Gt, "100", ColumnDisplayType::Number),
                comparison_filter("category", FilterComparisonOp::Eq, "Electronics", ColumnDisplayType::String),
            ],
            sort_keys: vec![],
            code_syntax_name: amalthea::comm::data_explorer_comm::CodeSyntaxName {
                code_syntax_name: "dplyr".to_string(),
            },
        };

        let resolved_sorts = vec![
            ResolvedSortKey {
                column_name: "sales".to_string(),
                ascending: false,
            },
            ResolvedSortKey {
                column_name: "region".to_string(),
                ascending: true,
            },
        ];

        let result = convert_to_code(params, Some("my_data"), &resolved_sorts);

        assert_eq!(result.converted_code, vec![
            "library(dplyr)".to_string(),
            "".to_string(),
            "my_data |>\n  filter(\n    price > 100,\n    category == \"Electronics\"\n  ) |>\n  arrange(desc(sales), region)".to_string(),
        ]);
    }

    #[test]
    fn test_default_object_name() {
        let params = ConvertToCodeParams {
            column_filters: vec![],
            row_filters: vec![],
            sort_keys: vec![],
            code_syntax_name: amalthea::comm::data_explorer_comm::CodeSyntaxName {
                code_syntax_name: "dplyr".to_string(),
            },
        };

        let result = convert_to_code(params, None, &[]);

        assert_eq!(result.converted_code, vec![
            "library(dplyr)".to_string(),
            "".to_string(),
            "dat".to_string(),
        ]);
    }
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
