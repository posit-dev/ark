//
// dap_variables.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark_test::DummyArkFrontend;

#[test]
fn test_dap_variables() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  x <- 42
  y <- 'hello'
  browser()
})
",
    );
    dap.recv_stopped();

    // Get the stack trace to get frame_id
    let stack = dap.stack_trace();
    assert!(stack.len() >= 1, "Expected at least 1 frame");
    let frame_id = stack[0].id;

    // Get scopes for the frame
    let scopes = dap.scopes(frame_id);
    assert!(!scopes.is_empty(), "Expected at least 1 scope");

    // Get the variables reference from the first (Locals) scope
    let variables_reference = scopes[0].variables_reference;
    assert!(
        variables_reference > 0,
        "Expected positive variables_reference"
    );

    // Get variables
    let variables = dap.variables(variables_reference);

    // Find x and y in the variables
    let x_var = variables.iter().find(|v| v.name == "x");
    let y_var = variables.iter().find(|v| v.name == "y");

    assert!(x_var.is_some(), "Expected variable 'x' in scope");
    assert!(y_var.is_some(), "Expected variable 'y' in scope");

    let x_var = x_var.unwrap();
    let y_var = y_var.unwrap();

    assert_eq!(x_var.value, "42");
    assert_eq!(y_var.value, "\"hello\"");

    frontend.debug_send_quit();
    dap.recv_continued();
}
