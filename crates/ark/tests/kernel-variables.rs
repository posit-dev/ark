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
