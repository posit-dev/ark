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

# Run clippy
clippy:
  cargo clippy --workspace --all-targets --all-features -- -D warnings

# Reformat source files
format:
  #!/usr/bin/env bash
  set -euo pipefail
  cargo +nightly fmt --all
  # Fall back to the Air binary bundled with the Positron or VS Code extension
  air=$(command -v air || ls -t ~/.positron/extensions/posit.air-vscode-*/bundled/bin/air ~/.vscode/extensions/posit.air-vscode-*/bundled/bin/air 2>/dev/null | head -1 || true)
  if [ -z "$air" ]; then
    echo "Air not found. Install it from https://posit-dev.github.io/air/cli.html" >&2
    exit 1
  fi
  "$air" format .
