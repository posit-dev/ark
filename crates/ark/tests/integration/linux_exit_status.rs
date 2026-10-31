// TEMPORARY: Deliberately failing tests to show that the Linux `atexit` hook in
// `DummyArkFrontend::init()` masks test failures by exiting with status 0.
// Expected: these FAIL on macOS and Windows, but PASS on Linux.

use anyhow::anyhow;
use ark_test::DummyArkFrontend;

#[test]
fn test_deliberate_failure_panic() {
    let _frontend = DummyArkFrontend::lock();
    panic!("Deliberate failure");
}

#[test]
fn test_deliberate_failure_err() -> anyhow::Result<()> {
    let _frontend = DummyArkFrontend::lock();
    Err(anyhow!("Deliberate failure"))
}

// Control: doesn't start the kernel, so the hook is never installed. Should
// FAIL on all platforms, including Linux.
#[test]
fn test_deliberate_failure_control() {
    panic!("Deliberate failure");
}
