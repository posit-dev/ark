//
// dap_assert.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use dap::types::Source;
use dap::types::StackFrame;

/// Assert a stack frame matches expected defaults for a virtual document frame.
#[track_caller]
pub fn assert_vdoc_frame(frame: &StackFrame, name: &str, line: i64, end_column: i64) {
    let StackFrame {
        id: 0,
        name: frame_name,
        source:
            Some(Source {
                name: Some(source_name),
                path: Some(path),
                source_reference: None,
                presentation_hint: None,
                origin: None,
                sources: None,
                adapter_data: None,
                checksums: None,
            }),
        line: frame_line,
        column: 1,
        end_line: Some(frame_end_line),
        end_column: Some(frame_end_column),
        can_restart: None,
        instruction_pointer_reference: None,
        module_id: None,
        presentation_hint: None,
    } = frame
    else {
        panic!("Frame doesn't match expected structure: {frame:#?}");
    };

    assert_eq!(frame_name, name);
    assert_eq!(*frame_line, line);
    assert_eq!(*frame_end_line, line);
    assert_eq!(*frame_end_column, end_column);
    assert_eq!(source_name, &format!("{name}.R"));
    assert!(path.starts_with("ark:"), "Expected ark: URI, got {path}");
    assert!(
        path.ends_with(&format!("{name}.R")),
        "Expected path ending with {name}.R, got {path}"
    );
}

/// Assert a stack frame matches expected values for a file-based frame.
#[track_caller]
pub fn assert_file_frame(frame: &StackFrame, path: &str, line: i64, end_column: i64) {
    let source = frame.source.as_ref().expect("Expected source");
    let frame_path = source.path.as_ref().expect("Expected path");

    assert!(
        frame_path.ends_with(path),
        "Expected path ending with {path}, got {frame_path}"
    );
    assert_eq!(frame.line, line, "line mismatch");
    assert_eq!(frame.end_line, Some(line), "end_line mismatch");
    assert_eq!(frame.end_column, Some(end_column), "end_column mismatch");
}
