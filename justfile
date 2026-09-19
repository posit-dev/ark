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

# Run the diagnostics benchmark without retaining a Criterion baseline.
bench *ARGS:
  @cargo bench --quiet -p ark --bench diagnostics -- --quiet --discard-baseline --warm-up-time 1 --measurement-time 3 {{ARGS}}

# Run with explicit Criterion comparison arguments, for example:
# `just bench-compare -- --save-baseline main` or
# `just bench-compare -- --baseline main`.
bench-compare *ARGS:
  @cargo bench --quiet -p ark --bench diagnostics {{ARGS}}

# Populate `target/bench-fixtures/` with the pinned dplyr corpus and its CRAN
# imports. This is the only benchmark command that uses the network.
bench-fixtures:
  @cargo bench --quiet -p ark --bench diagnostics -- --populate

# Run clippy
clippy:
  cargo clippy --workspace --all-targets --all-features -- -D warnings

# Reformat source files
format:
  cargo +nightly fmt --all
