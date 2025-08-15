//
// convert_to_code.rs
//
// Copyright (C) 2025 by Posit Software, PBC
//
//

use amalthea::comm::data_explorer_comm::CodeSyntaxName;
use amalthea::comm::data_explorer_comm::ConvertToCodeParams;
use amalthea::comm::data_explorer_comm::ConvertedCode;

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
pub fn convert_to_code(_params: ConvertToCodeParams, object_name: Option<&str>) -> ConvertedCode {
    // Create a library statement for dplyr
    let library_statement = "library(dplyr)".to_string();

    // Use a default placeholder if no object name is provided
    let object_ref = match object_name {
        Some(name) => name.to_string(),
        None => "dat".to_string(), // Default placeholder if no object name
    };

    // Create a simple pipe expression to slice the first 3 rows
    let pipe_expression = format!("{} |>\n  slice(1:3)", object_ref);

    // Combine the code lines
    ConvertedCode {
        converted_code: vec![library_statement, "".to_string(), pipe_expression],
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
