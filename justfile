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

# See `crates/oak_db/fuzz/README.md` for testing responsibilities and workflows.
# Fuzz Salsa cycle handling across workspaces, entry queries, and edit histories
fuzz:
  cargo nextest run --no-fail-fast -p oak_db --run-ignored only -E 'test(/^tests::fuzz::test_block_/)'

# `SEED` accepts any decimal `u64`; `0` through `5` are the standard suite blocks.
# Replay one deterministic fuzz block with operation tracing
fuzz-replay-seed SEED:
  OAK_FUZZ_SEED={{SEED}} OAK_FUZZ_TRACE=1 cargo nextest run --no-capture -p oak_db --run-ignored only -E 'test(=tests::fuzz::test_replay_block)'

# `PATH` is relative to the repository root, while the test runs in `crates/oak_db`.
# Replay one saved scenario with operation tracing
fuzz-replay-scenario PATH:
  OAK_FUZZ_SCENARIO={{quote(absolute_path(PATH))}} OAK_FUZZ_TRACE=1 cargo nextest run --no-capture -p oak_db --run-ignored only -E 'test(=tests::fuzz::test_replay_scenario)'

# `PATH` is relative to the repository root, while the test runs in `crates/oak_db`.
fuzz-replay-semantic PATH:
  OAK_FUZZ_SCENARIO={{quote(absolute_path(PATH))}} OAK_FUZZ_TRACE=1 cargo nextest run --no-capture -p oak_db --run-ignored only -E 'test(=tests::fuzz::test_replay_semantic_scenario)'

# `PATH` is relative to the repository root, while the test runs in
# `crates/oak_db`. Tracing is off because shrinking can compare hundreds of
# candidates. The final reduced scenario is reported.
# Reduce a saved semantic mismatch
fuzz-reduce-semantic PATH:
  OAK_FUZZ_SCENARIO={{quote(absolute_path(PATH))}} cargo nextest run --no-capture -p oak_db --run-ignored only -E 'test(=tests::fuzz::test_reduce_semantic_scenario)'

# `DIR` is relative to the repository root, while the test runs in `crates/oak_db`.
# Compare each JSON scenario in a corpus against a fresh database
fuzz-sweep-semantic DIR:
  OAK_FUZZ_CORPUS={{quote(absolute_path(DIR))}} cargo nextest run --no-capture -p oak_db --run-ignored only -E 'test(=tests::fuzz::test_sweep_semantic_corpus)'

# Write the deterministic seed corpus to disk for the cargo-fuzz driver
fuzz-corpus:
  OAK_FUZZ_CORPUS=fuzz/corpus/scenario cargo nextest run --no-capture -p oak_db --run-ignored only -E 'test(=tests::fuzz::test_write_seed_corpus)'

# ASan completes fewer than 50 runs in five minutes here, versus about 500 per
# second without it, likely because `stacker::maybe_grow()` switches stacks.
# `-s none` disables ASan but retains coverage instrumentation.
# `-timeout` overrides libFuzzer's 1200s default.
# Run coverage-guided fuzzing without AddressSanitizer
fuzz-explore: fuzz-corpus
  cd crates/oak_db/fuzz && cargo +nightly fuzz run -s none scenario corpus/scenario -- -runs=100000 -max_len=32768 -timeout=20

# Run clippy
clippy:
  cargo clippy --workspace --all-targets --all-features -- -D warnings

# Reformat source files
format:
  cargo +nightly fmt --all
