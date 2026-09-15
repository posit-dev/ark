# Fuzzing Oak query cycles

This runbook covers local commands, CI, corpus maintenance, and failure replay. See the [`oak_db::fuzz` module](../src/fuzz.rs) for the scenario model and query coverage, and [the test entry points](../src/tests/fuzz.rs) for ordinary checks.

## Property and coverage

The generic runner requires every scenario to finish without panicking or hanging. It does not compare query results with expected values or compare an edited database with a fresh one. Incorrect exports or diagnostics can pass.

Recovery logs identify which handlers ran, but not which query was Salsa's repeated key. That key depends on query entry order. Focused regression tests assert particular recovery firings and semantic behavior separately from the mutated scenarios.

Workspaces can also model packages: a `Workspace`-kind package owns its own root and files, and a `Library`-kind package sits in the library root with a synthetic NAMESPACE but no files. Mutation grows and shrinks their exports and `importFrom` re-exports, so a scenario can chain packages into an acyclic lookup, a mutual re-export cycle that drives `Package::resolve()`'s `cycle_result` handler, or a consumer path through a `library()` attach or a package's own re-exports. Library packages never own files, so a chain can only terminate at a local definition through a `Workspace`-kind package.

## Mutation budgets and replay limits

[Generation budgets](../src/fuzz/budgets.rs) keep scenarios small enough for fast checks. [Replay limits](../src/fuzz/limits.rs) independently bound accepted artifacts, including edit replacements. Keep replay limits stable when tuning generation budgets so saved failures remain replayable. Statement and text limits leave room for one insertion to cross a growth threshold.

[The mutator](../src/fuzz/mutate.rs) separates sampling probabilities from choice vocabularies. It can add both workspace and library packages, and add files to loose scripts or any workspace package. [Shared traversal](../src/fuzz/traversal.rs) addresses initial programs and edit replacements for both mutation and validation.

## Prerequisites

``` sh
rustup toolchain install nightly
cargo install cargo-fuzz --locked
```

The ordinary test suite also needs `just` and `cargo-nextest`. CI targets Linux. The driver can also run locally on macOS.

The driver uses `-s none` to retain coverage instrumentation without AddressSanitizer. Local measurements found much lower throughput with ASan, possibly related to stack switching in `stacker::maybe_grow()`. This choice limits detection of memory errors.

## Seeds and local exploration

Run these commands from the repository root.

``` sh
just fuzz-corpus
just fuzz-driver
```

`just fuzz-corpus` writes named regressions and generated seed variants as JSON under `crates/oak_db/fuzz/corpus/scenario/`. The Rust fixtures are the source of these inputs. The generated directory is ignored by Git.

`just fuzz-driver` regenerates the seeds and starts coverage-guided search. New inputs accumulate in the same directory. The custom mutator decodes JSON and mutates structured scenarios, with a byte-mutation fallback when decoding or mutation fails or the result exceeds the available buffer.

## Replay

From the repository root, replay a saved scenario with operation tracing.

``` sh
just fuzz-replay path/to/scenario.json
```

This uses the repository's stable toolchain and needs neither nightly nor cargo-fuzz. Relative paths are resolved from the repository root.

To reproduce through the adapter, run this from `crates/oak_db/fuzz`, with the input path relative to that directory.

``` sh
cargo +nightly fuzz run -s none scenario path/to/input -- -timeout=20
```

A `timeout-*` file exceeded the original run's time limit. It may finish on another machine or under another build. Use the same timeout when comparing runs. Read the operation report before using a debugger to investigate the query in progress.

## Minimization

Run these commands from `crates/oak_db/fuzz`.

- Reduce one failing input with `cargo +nightly fuzz tmin -s none scenario artifacts/scenario/crash-<hash>`.
- Reduce the coverage corpus with `cargo +nightly fuzz cmin -s none scenario corpus/scenario -- -timeout=20`. This removes coverage-redundant inputs and writes retained inputs under hash filenames. Regenerate named seeds before the next exploration run.

Stopping reduction does not establish minimality. Replay the reduced input in a fresh process and compare the failure and operation in progress with the original. Reduction can move a failure onto a different code path. When investigating a timeout, preserve its per-input time limit during reduction.

In cargo-fuzz 0.13.2, `cmin` can print `Failed to minimize corpus` and exit 0, leaving the original corpus in place. libFuzzer can also continue merging after an individual input fails. CI checks both the log and newly written artifacts before accepting the result. Check them after local minimization too.

## CI

The [fuzz workflow](../../../.github/workflows/test-fuzz.yml) uses these budgets.

| Event           | Ordinary blocks | Stable adapter check | Driver | Search time |
|-----------------|-----------------|----------------------|--------|-------------|
| Pull request    | yes             | yes                  | no     | --          |
| Push to main    | yes             | yes                  | yes    | 300s        |
| Daily schedule  | no              | no                   | yes    | 1800s       |
| Manual dispatch | yes             | yes                  | yes    | 300s        |

Run `just fuzz-driver` locally when changing the adapter, `Scenario` serialization, or `ScenarioMutator`. PR CI type-checks the adapter without nightly but does not run libFuzzer.

Scheduled runs skip the ordinary blocks because their fixed seeds repeat the same mutation sequences. Both exploration and minimization allow 20 seconds per input. Minimization has a ten-minute step limit, and the entire driver job has a sixty-minute limit, including setup and compilation. Step timings separate build, search, and minimization costs.

## Corpus cache

Each driver run restores the previous corpus, adds the current seeds, explores, minimizes, and saves. Cache keys are `oak-fuzz-corpus-v2-<run id>-<run attempt>`, restored with the `oak-fuzz-corpus-v2-` prefix.

- Bump the version in both keys and the restore prefix when the seed shape changes enough that the accumulated corpus is worth rebuilding, not only when old `Scenario` JSON stops decoding. The legacy JSON adapter keeps older saved inputs replayable regardless.
- Reset exploration by bumping the version or deleting the caches. Regression seeds are regenerated on every run, including after cache eviction or `cmin`.
- A concurrency group permits one writer per ref, so overlapping runs do not independently extend the same corpus and discard each other's discoveries.
- Runs on other refs can restore the default branch's cache but save in their own cache scope. They do not update the main branch's corpus.
- A failed exploration or minimization step prevents the save, leaving the previous cache available for the next run.

## Failure artifacts

Download the artifact archive from the workflow run's summary page.

- `fuzz-driver-artifacts` records failures before minimization.
- `fuzz-cmin-artifacts` records failures during minimization.

Depending on how far the run progressed, the archive contains these files. Paths below identify their locations or filenames within the archive.

| File                                      | Contents                                                                                                                |
|-------------------------------------------|-------------------------------------------------------------------------------------------------------------------------|
| `artifacts/scenario/crash-*`, `timeout-*` | Concrete inputs for replay.                                                                                             |
| `target/oak_fuzz/*.artifact`              | Rendered scenario and a `current:` line naming the operation in progress.                                               |
| `provenance.txt`                          | Revision, event, budget, and compiler and cargo-fuzz versions. The exploration seed is appended after a successful run. |
| `explore.log`                             | Exploration output, including its seed and coverage counters.                                                           |
| `cmin.log`                                | Minimization output, included in the minimization-failure archive.                                                      |

Start with the recorded revision and replay the concrete input. Use the operation report to locate the query or edit in progress. The report is written before each operation, so it can remain available even when the process terminates without writing a libFuzzer input artifact. An ordinary libFuzzer-handled abort can still produce a concrete input.

## Promoting a failure into a regression

Preserve regressions in the repository so they survive cache resets.

1.  Reduce the saved input and confirm it still fails for the relevant reason.
2.  Express it as a named scenario in [`fuzz::corpus`](../src/fuzz/corpus.rs), using the `build` helpers. Compare it with the saved input to check that the transcription preserves the behavior.
3.  Add an ordinary test with the expected result or recovery assertions.

The named scenario becomes both a regression fixture and a mutation seed. `just fuzz-corpus` writes it into subsequent exploration runs.
