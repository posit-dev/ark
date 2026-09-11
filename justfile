# Run the tests
test *ARGS:
  cargo nextest run --no-fail-fast {{ARGS}}

# Run the tests in verbose mode
# `--no-capture` forces stdout/stderr to be shown for all tests, not just failing ones,
# and also forces them to be run sequentially so you don't see interleaved live output
test-verbose:
  cargo nextest run --no-capture

# Run the insta tests in update mode
test-insta:
  cargo insta test --test-runner nextest

# Rewrite the diagnostic snapshots in place
test-insta-diagnostics:
  INSTA_UPDATE=always cargo nextest run -p oak_db test_diagnostic_

# Vary cold query entry points and edit histories to expose Salsa cycle panics
fuzz:
  cargo nextest run --no-fail-fast -p oak_db --run-ignored only -E 'test(/^tests::fuzz::test_seeds_/)'

# Report each operation eagerly because hangs cannot produce an unwind report
fuzz-seed SEED:
  OAK_FUZZ_SEED={{SEED}} OAK_FUZZ_TRACE=1 cargo nextest run --no-capture -p oak_db --run-ignored only -E 'test(=tests::fuzz::test_replay_seed)'

# Run clippy
clippy:
  cargo clippy --workspace --all-targets --all-features -- -D warnings

# Reformat source files
format:
  cargo +nightly fmt --all
