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

#[test]
fn test_dap_variables_nested() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  my_list <- list(a = 1, b = 2, c = list(nested = 'deep'))
  my_df <- data.frame(x = 1:3, y = c('a', 'b', 'c'))
  browser()
})
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;
    let scopes = dap.scopes(frame_id);
    let variables = dap.variables(scopes[0].variables_reference);

    // Find my_list - it should have a variables_reference > 0 for expansion
    let list_var = variables.iter().find(|v| v.name == "my_list").unwrap();
    assert!(
        list_var.variables_reference > 0,
        "List should be expandable (variables_reference > 0)"
    );

    // Expand the list to see its children
    let list_children = dap.variables(list_var.variables_reference);
    assert!(
        list_children.len() >= 3,
        "List should have at least 3 children"
    );

    let a_child = list_children.iter().find(|v| v.name == "a").unwrap();
    assert_eq!(a_child.value, "1");

    let c_child = list_children.iter().find(|v| v.name == "c").unwrap();
    assert!(
        c_child.variables_reference > 0,
        "Nested list 'c' should be expandable"
    );

    // Expand nested list
    let nested_children = dap.variables(c_child.variables_reference);
    let nested_var = nested_children.iter().find(|v| v.name == "nested").unwrap();
    assert_eq!(nested_var.value, "\"deep\"");

    // Find my_df - data frames are classed objects, currently shown as class name
    // but not expandable (this is current behavior)
    let df_var = variables.iter().find(|v| v.name == "my_df").unwrap();
    assert!(
        df_var.value.contains("data.frame"),
        "Data frame should show class name"
    );

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_variables_types() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  v_int <- 42L
  v_dbl <- 3.14
  v_chr <- 'hello'
  v_lgl <- TRUE
  v_null <- NULL
  v_na <- NA
  v_vec <- c(1, 2, 3)
  v_factor <- factor(c('a', 'b', 'a'))
  v_fn <- function(x) x + 1
  v_env <- new.env()
  browser()
})
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;
    let scopes = dap.scopes(frame_id);
    let variables = dap.variables(scopes[0].variables_reference);

    // Integer
    let v = variables.iter().find(|v| v.name == "v_int").unwrap();
    assert_eq!(v.value, "42L");

    // Double
    let v = variables.iter().find(|v| v.name == "v_dbl").unwrap();
    assert_eq!(v.value, "3.14");

    // Character
    let v = variables.iter().find(|v| v.name == "v_chr").unwrap();
    assert_eq!(v.value, "\"hello\"");

    // Logical
    let v = variables.iter().find(|v| v.name == "v_lgl").unwrap();
    assert_eq!(v.value, "TRUE");

    // NULL
    let v = variables.iter().find(|v| v.name == "v_null").unwrap();
    assert_eq!(v.value, "NULL");

    // NA
    let v = variables.iter().find(|v| v.name == "v_na").unwrap();
    assert_eq!(v.value, "NA");

    // Vector - shows formatted value (not expandable in current implementation)
    let v = variables.iter().find(|v| v.name == "v_vec").unwrap();
    assert!(
        v.value.contains("1") && v.value.contains("2") && v.value.contains("3"),
        "Vector should show formatted values"
    );

    // Factor - classed object, shows class name
    let v = variables.iter().find(|v| v.name == "v_factor").unwrap();
    assert!(v.value.contains("factor"), "Factor should show class name");

    // Function
    let v = variables.iter().find(|v| v.name == "v_fn").unwrap();
    assert!(
        v.value.contains("function"),
        "Function should show function signature"
    );

    // Environment - should be expandable (it's a VECSXP internally when captured)
    let v = variables.iter().find(|v| v.name == "v_env").unwrap();
    assert!(
        v.value.contains("environment"),
        "Environment should show type"
    );

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_variables_multiple_frames() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
outer_var <- 'outer_value'
outer <- function() {
  outer_local <- 'from_outer'
  inner()
}
inner <- function() {
  inner_local <- 'from_inner'
  browser()
}
outer()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert!(stack.len() >= 2, "Expected at least 2 frames");

    // Check inner frame (top of stack)
    let inner_frame_id = stack[0].id;
    let inner_scopes = dap.scopes(inner_frame_id);
    let inner_vars = dap.variables(inner_scopes[0].variables_reference);

    let inner_local = inner_vars.iter().find(|v| v.name == "inner_local");
    assert!(
        inner_local.is_some(),
        "inner_local should be in inner frame"
    );
    assert_eq!(inner_local.unwrap().value, "\"from_inner\"");

    // inner frame should NOT have outer_local
    let outer_in_inner = inner_vars.iter().find(|v| v.name == "outer_local");
    assert!(
        outer_in_inner.is_none(),
        "outer_local should NOT be in inner frame"
    );

    // Check outer frame
    let outer_frame_id = stack[1].id;
    let outer_scopes = dap.scopes(outer_frame_id);
    let outer_vars = dap.variables(outer_scopes[0].variables_reference);

    let outer_local = outer_vars.iter().find(|v| v.name == "outer_local");
    assert!(
        outer_local.is_some(),
        "outer_local should be in outer frame"
    );
    assert_eq!(outer_local.unwrap().value, "\"from_outer\"");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_variables_empty_scope() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Function with no local variables
    let _file = frontend.send_source(
        "
empty_fn <- function() {
  browser()
}
empty_fn()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;
    let scopes = dap.scopes(frame_id);

    // The scope might have variables_reference = 0 for empty scope,
    // or return an empty list
    if scopes[0].variables_reference > 0 {
        let variables = dap.variables(scopes[0].variables_reference);
        assert!(
            variables.is_empty(),
            "Empty function should have no local variables"
        );
    }
    // If variables_reference is 0, that's also valid for empty scope

    frontend.debug_send_quit();
    dap.recv_continued();
}
