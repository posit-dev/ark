# Run the tests
test +ARGS:
  cargo nextest run {{ARGS}}

# Run the tests in verbose mode
# `--no-capture` forces stdout/stderr to be shown for all tests, not just failing ones,
# and also forces them to be run sequentially so you don't see interleaved live output
test-verbose:
  cargo nextest run --no-capture

# Run the insta tests in update mode
test-insta:
  cargo insta test --test-runner nextest
