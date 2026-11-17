use ark_test::DummyArkFrontend;

#[test]
fn test_variables_last_value() {
    let frontend = DummyArkFrontend::lock();

    // Set up a global variable before opening the variables comm
    frontend.execute_request_invisibly("test_var <- 'hello'");

    // Open the variables comm and receive the initial Refresh
    let initial = frontend.open_variables_comm();
    let names: Vec<&str> = initial
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert!(names.contains(&"test_var"));
    assert!(!names.contains(&".Last.value"));

    // Request that Ark start showing `.Last.value`
    frontend.execute_request_invisibly("options(positron.show_last_value = TRUE)");

    // The variables pane should have sent an Update, and now `.Last.value` should be in there
    let update = frontend.recv_variables_update();
    let names: Vec<&str> = update
        .assigned
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert!(!names.contains(&"test_var"));
    assert!(names.contains(&".Last.value"));

    // Set up another variable holding `NULL`
    frontend.execute_request_invisibly("test_var2 <- NULL");

    // Should get new variable and `.Last.value` again
    let update = frontend.recv_variables_update();
    let names: Vec<&str> = update
        .assigned
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert!(!names.contains(&"test_var"));
    assert!(names.contains(&"test_var2"));
    assert!(names.contains(&".Last.value"));

    // Set up another variable holding `NULL` a second time in a row
    frontend.execute_request_invisibly("test_var3 <- NULL");

    // Should get new variable but NOT `.Last.value` again, it didn't change! Same pointer!
    let update = frontend.recv_variables_update();
    let names: Vec<&str> = update
        .assigned
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert!(!names.contains(&"test_var"));
    assert!(!names.contains(&"test_var2"));
    assert!(names.contains(&"test_var3"));
    assert!(!names.contains(&".Last.value"));

    // Request that Ark stop showing `.Last.value`
    frontend.execute_request_invisibly("options(positron.show_last_value = FALSE)");

    // `.Last.value` should get removed
    let update = frontend.recv_variables_update();
    assert!(update.removed.contains(&String::from(".Last.value")));
}

// https://github.com/posit-dev/positron/issues/13294
#[test]
fn test_readr_read_csv_does_not_crash_variables_pane() {
    let frontend = DummyArkFrontend::lock();

    if !frontend.is_installed("readr") {
        println!("Skipping test: readr package not installed");
        return;
    }

    let csv_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/test-issue-13294.csv"
    )
    .replace('\\', "/");

    let initial = frontend.open_variables_comm();
    assert!(initial.variables.is_empty());

    // This triggers the variables pane to compute `r_size()` on the tibble
    // produced by readr, which has deeply nested ALTREP internals.
    let code = format!("x <- readr::read_csv('{csv_path}', show_col_types = FALSE)");
    frontend.send_execute_request(&code, Default::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    let update = frontend.recv_variables_update();
    assert!(update.assigned.iter().any(|v| v.display_name == "x"));

    // Exercise `r_size` directly via `.ps.internal(obj_size())`
    frontend.execute_request(".ps.internal(obj_size(x))", |_result| {});
}

// https://github.com/posit-dev/positron/issues/17279
#[test]
fn test_variables_pane_does_not_share_altrep_wrapper_data() {
    let frontend = DummyArkFrontend::lock();

    // `drop()` returns an ALTREP wrapper for vectors of length >= 64
    frontend.execute_request_invisibly("x <- drop(matrix(as.double(1:64), ncol = 1))");

    let initial = frontend.open_variables_comm();
    assert_eq!(initial.variables[0].display_name, "x");

    // The first use locks the wrapper's data. The second use errors with
    // "REFCNT on locked WRAPPER data increased to 2" on R >= 4.6.2 if the
    // variables pane holds a reference to that data.
    frontend.execute_request_invisibly("a <- x + 1");
    frontend.recv_variables_update();
    frontend.execute_request_invisibly("b <- x + 2");
    frontend.recv_variables_update();
}
