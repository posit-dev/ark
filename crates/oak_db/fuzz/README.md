# Fuzzing Oak query cycles

The fuzz suite exercises Salsa queries across generated workspaces and edit histories. See [`oak_db::fuzz`](../src/fuzz.rs) for the scenario model and query coverage, and [the test entry points](../src/tests/fuzz.rs) for deterministic checks.

This crate is a nested workspace, so nothing outside `cargo +nightly fuzz` builds it. [`fuzz_targets/scenario.rs`](fuzz_targets/scenario.rs) therefore holds only the `libfuzzer-sys` macro shells and delegates to [`oak_db::fuzz::driver`](../src/fuzz/driver.rs), where the byte-buffer decisions are covered by tests and clippy.

## What it checks

Each scenario must finish without panicking or hanging. The runner does not compare query results with expected values or compare an edited database with a fresh one, so incorrect exports or diagnostics can pass. Focused regression tests remain responsible for semantic assertions.

Scenarios cover source cycles, package re-export cycles, and consumer paths into both. Recovery logs identify which handlers ran, but not which query was Salsa's repeated key, which depends on query entry order.

Coverage-guided exploration repeatedly mutates scenarios and observes which code paths each input executes. It keeps inputs that reach previously unexplored code, then mutates those inputs further. The goal is to discover query entry orders, dependency graphs, and edit sequences that fixed tests did not anticipate and that expose cycle panics or hangs. A weekly CI job resumes from the saved corpus to advance this search over time.

## Testing responsibilities

| Check                   | Purpose                                                                                                           | Runs automatically                                                             | Run locally                                                                                                          |
|-------------------------|-------------------------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------|----------------------------------------------------------------------------------------------------------------------|
| Ordinary `oak_db` tests | Assert semantic results and specific recovery behavior. Includes a short fuzz smoke test.                         | Pull requests and pushes to main.                                              | While developing affected behavior.                                                                                  |
| `just fuzz`             | Run bounded, fixed-seed mutation blocks. Detects panics and hangs reproducibly but does not verify query results. | Pull requests, pushes to main, and manual fuzz workflows.                      | After changing an `oak_db` query, cycle handler, or dependency path.                                                 |
| `just fuzz-explore`     | Use coverage feedback to discover new execution paths and grow the saved corpus.                                  | Pushes to main, weekly schedules, and manual fuzz workflows. Not pull requests. | After changing scenario serialization or the mutator: pull request CI type-checks the driver but never runs it.     |
| Replay and minimization | Reproduce, diagnose, and reduce a discovered failure.                                                             | Never.                                                                         | When deterministic fuzzing or exploration finds a failure.                                                           |

## Prerequisites

Deterministic checks require `just` and `cargo-nextest`. Coverage-guided exploration also requires:

``` sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked
```

CI runs on Linux. The driver also runs locally on macOS.

The driver uses `-s none` to retain coverage instrumentation without AddressSanitizer. ASan produced much lower throughput in local measurements, possibly because `stacker::maybe_grow()` switches stacks. Disabling it limits detection of memory errors.

## Run locally

Run commands from the repository root:

``` sh
just test -p oak_db
just fuzz
just fuzz-explore
```

`just fuzz-explore` regenerates the seed corpus before starting libFuzzer. Run `just fuzz-corpus` separately only to inspect the generated seeds or before invoking `cargo fuzz` directly.

Generated and discovered inputs accumulate under `crates/oak_db/fuzz/corpus/scenario/`. Named Rust fixtures are the source of regression seeds, and the generated directory is ignored by Git.

## Replay

`just fuzz` splits deterministic checking into independently runnable blocks. Each block uses one seed for both its starting corpus and mutation sequence. Replay a block with operation and recovery tracing:

``` sh
just fuzz-replay-seed 0
```

`SEED` accepts any decimal `u64`.

Replay a saved scenario:

``` sh
just fuzz-replay-scenario path/to/scenario.json
```

Both replay commands use the stable toolchain. Scenario paths are resolved from the repository root.

To reproduce through the libFuzzer adapter, run this from `crates/oak_db/fuzz`, with the input path relative to that directory:

``` sh
cargo +nightly fuzz run -s none scenario path/to/input -- -timeout=20
```

A `timeout-*` artifact exceeded the original per-input limit but may finish under another build or on another machine. Preserve that limit when comparing runs, and inspect the operation report to identify the query or edit in progress.

## Minimize

Once fuzzing finds a regression, minimize the failing input before diagnosing or promoting it. `tmin` removes input while preserving the failure. `cmin` serves a separate maintenance purpose: it removes corpus entries that add no unique coverage, keeping later exploration fast.

Run these commands from `crates/oak_db/fuzz`:

``` sh
cargo +nightly fuzz tmin -s none scenario artifacts/scenario/crash-<hash>
cargo +nightly fuzz cmin -s none scenario corpus/scenario -- -timeout=20
```

`cmin` replaces retained filenames with hashes, so regenerate named seeds before further exploration.

Replay a reduced input in a fresh process and confirm that it still fails for the same reason. Reduction can move a failure to another code path. Preserve the per-input timeout when reducing a hang.

In cargo-fuzz 0.13.2, `cmin` can print `Failed to minimize corpus` and exit successfully, and libFuzzer can continue merging after an input fails. Check both the output and `artifacts/scenario/`; CI checks both before saving a minimized corpus.

## CI and corpus retention

The [fuzz workflow](../../../.github/workflows/test-fuzz.yml) runs a five-minute exploration after pushes to main and manual dispatches, and a thirty-minute exploration each week. A scenario that runs for twenty seconds is treated as a timeout. The workflow reserves additional time for compilation and corpus minimization.

Each successful exploration restores the previous corpus, adds current seeds, explores, minimizes, and saves the result. A failed exploration or minimization leaves the previous cache intact.

Bump the cache version in `.github/workflows/test-fuzz.yml` when the seed shape changes enough to justify discarding accumulated coverage. A saved input that no longer decodes is skipped rather than repaired. Named regression seeds are regenerated on every run.

## Failure artifacts

Download the artifact archive from the workflow run's summary page:

- `fuzz-driver-artifacts` records failures before minimization.
- `fuzz-cmin-artifacts` records minimization failures.

Depending on where the run failed, the archive contains:

| File                                      | Contents                                                                     |
|-------------------------------------------|------------------------------------------------------------------------------|
| `artifacts/scenario/crash-*`, `timeout-*` | Concrete inputs for replay.                                                  |
| `target/oak_fuzz/*.artifact`              | The rendered scenario and operation in progress.                             |
| `provenance.txt`                          | Revision, event, budget, tool versions, and exploration seed when available. |
| `explore.log`                             | Exploration seed and coverage counters.                                      |
| `cmin.log`                                | Minimization output.                                                         |

Start from the recorded revision and replay the concrete input. The operation report is written before each operation, so it can survive a termination that produces no libFuzzer input artifact.

## Turn a fuzz failure into a regression test

When fuzzing finds a reproducible failure, preserve it as a deterministic test:

1. Reduce the saved input and confirm that it still fails for the same reason.
2.  Express it as a named scenario in [`fuzz::corpus`](../src/fuzz/corpus.rs), using the `build` helpers.
3.  Add a deterministic test with the expected result or recovery assertions.

The named scenario becomes both a regression fixture and a mutation seed. `just fuzz-corpus` includes it in later exploration runs.
