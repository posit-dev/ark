//
// convert_to_code.rs
//
// Copyright (C) 2024 by Posit Software, PBC
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
///
/// # Returns
///
/// A `ConvertedCode` containing lines of code implementing the filters and sort keys
pub fn convert_to_code(_params: ConvertToCodeParams) -> ConvertedCode {
    // For now, just return a simple stub message
    ConvertedCode {
        converted_code: vec!["here's some dplyr code".to_string()],
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
