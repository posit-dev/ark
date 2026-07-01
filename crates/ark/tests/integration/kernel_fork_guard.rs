use ark_test::DummyArkFrontend;

// `parallel` ships with base R, so no extra test dependency is needed.
//
// The fork guard only installs on Unix, because forking is the hazard and
// Windows `parallel` never forks (it ships its own serial/`stop()` stubs in
// `R/windows/mcdummies.R`). So the error tests below are `#[cfg(unix)]`: on
// Windows these calls hit R's native behavior, which uses different messages
// and even runs serially in some cases. The PSOCK tests stay cross-platform.

#[cfg(unix)]
#[test]
fn test_fork_functions_error() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_error(
        "parallel::mclapply(1:2, identity, mc.cores = 2)",
        |error_msg| {
            assert!(error_msg.contains("fork the R session"));
        },
    );

    // `library(parallel)` attaches the package, exercising the `pkg_bind` /
    // attach path. Unqualified `mclapply` should then hit the shim too.
    frontend.execute_request_error("library(parallel); mclapply(1:2, identity)", |error_msg| {
        assert!(error_msg.contains("fork the R session"));
    });

    frontend.execute_request_error("parallel::mcparallel(1)", |error_msg| {
        assert!(error_msg.contains("fork the R session"));
    });

    frontend.execute_request_error("parallel::pvec(1:10, sqrt, mc.cores = 2)", |error_msg| {
        assert!(error_msg.contains("fork the R session"));
    });

    frontend.execute_request_error(
        "parallel::mcmapply(`+`, 1:2, 3:4, mc.cores = 2)",
        |error_msg| {
            assert!(error_msg.contains("fork the R session"));
        },
    );

    frontend.execute_request_error("parallel::mcMap(`+`, 1:2, 3:4)", |error_msg| {
        assert!(error_msg.contains("fork the R session"));
    });

    frontend.execute_request_error("parallel::makeForkCluster(2)", |error_msg| {
        assert!(error_msg.contains("fork the R session"));
    });

    // `mcfork` is unexported, so triple-colon access exercises the
    // namespace-only binding.
    frontend.execute_request_error("parallel:::mcfork()", |error_msg| {
        assert!(error_msg.contains("fork the R session"));
    });

    // The public `makeCluster(type = "FORK")` dispatches to `makeForkCluster`
    // resolved through parallel's namespace, so it hits our shim too.
    frontend.execute_request_error("parallel::makeCluster(2, type = \"FORK\")", |error_msg| {
        assert!(error_msg.contains("fork the R session"));
    });
}

// The multicore family runs serially, without forking, when `mc.cores = 1` or
// the input has fewer than two elements. Those calls never reach `mcfork()`, so
// the guard must leave them alone. This matches Windows, where {parallel} always
// runs these serially.

#[test]
fn test_serial_execution_works() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request(
        "unlist(parallel::mclapply(1:4, function(x) x * 2L, mc.cores = 1))",
        |result| assert_eq!(result, "[1] 2 4 6 8"),
    );

    frontend.execute_request(
        "parallel::pvec(1:4, function(x) x * 2L, mc.cores = 1)",
        |result| assert_eq!(result, "[1] 2 4 6 8"),
    );
}

// PSOCK clusters start fresh R worker processes instead of forking the session,
// so the guard must leave them untouched. These spawn real worker processes and
// run a small parallel computation, asserting it returns the correct result.

#[test]
fn test_psock_cluster_works() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request(
        "local({ cl <- parallel::makePSOCKcluster(2); on.exit(parallel::stopCluster(cl)); sum(unlist(parallel::parLapply(cl, 1:4, function(x) x * 2L))) })",
        |result| assert_eq!(result, "[1] 20"),
    );

    // `makeCluster()` defaults to a PSOCK cluster, which we don't guard.
    frontend.execute_request(
        "local({ cl <- parallel::makeCluster(2); on.exit(parallel::stopCluster(cl)); unlist(parallel::clusterApply(cl, 1:3, function(x) x + 1L)) })",
        |result| assert_eq!(result, "[1] 2 3 4"),
    );
}
