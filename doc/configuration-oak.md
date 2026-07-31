# Introduction

Oak is the semantic engine behind Ark's intellisense. It indexes R code per file (scopes, symbols, definitions, uses) and answers analysis questions for the LSP: diagnostics, go-to-definition, etc.

For Ark's general configuration, see [configuration.md](configuration.md).

# Precedence

Configuration follows these rules of precedence:

1. Environment variables.
2. LSP settings from the client.
3. Defaults.

Environment variables read `1` and `true` as on, `0` and `false` as off, case-insensitively. Anything else, including the empty string, counts as unset and falls through to the setting.

# LSP settings

## `oak.sourceFetching.enabled`

A boolean, default `TRUE`.

Whether to recover the R sources of the packages your workspace uses, so analysis can see inside them. See [Package sources](#package-sources) for what that involves.

Takes effect immediately. Turning it back on fetches sources for the packages Oak has already seen, without needing a restart.

Overridden by the `OAK_SOURCE_FETCHING_ENABLED` environment variable.

To avoid unnecessary traffic on CI, source fetching is disabled when the `CI` environment variable is set. Set `OAK_SOURCE_FETCHING_ENABLED=1` to opt back into fetching sources in a CI job.

# Package sources

When Oak detects that your code uses an external package, it tries to recover that package's R sources to index them and infer types and effects. It stops at the first thing that works:

1. **Base packages** (`base`, `stats`, `utils`, and friends) come from a prebuilt archive hosted as a GitHub Release at [`posit-dev/oak-r-sources`](https://github.com/posit-dev/oak-r-sources), one release per R version. R's own source tarball is over 100MB per version, so we publish a trimmed, zstd-compressed `r-source.tar.zst` that's about 1.7MB instead.

2. **Local srcrefs.** If a package was installed with srcrefs, the sources are already on your machine. Oak recovers them by running a short script in a sidecar R process. If you maintain a package, consider adding `KeepSource: true` to your DESCRIPTION file. This compels R to always install your package with sources, which is useful for both debugging and language analysis.

3. **A CRAN tarball**, downloaded and unpacked.

If none of those work, or if source fetching is disabled, Oak analysis degrades gracefully.

## Cache

Sources recovered from srcrefs or from a download are cached on disk and shared across sessions.

The cache lives in `~/.cache/oak/` on macOS and Linux, and in
`%LOCALAPPDATA%\oak\` on Windows. Laid out as:

```
oak/
  source/v1/cran/{name}_{version}/    unpacked CRAN tarballs
  source/v1/r/{release}_archive/      the downloaded r-source.tar.zst
  source/v1/r/{release}_{version}/    base R sources unpacked from it
  srcref/v1/{name}_{version}_{hash}/  sources recovered from local srcrefs
```

The `v1` is a cache format version.

The srcref key includes a hash of the package's `Built:` field, so reinstalling or rebuilding a package produces a fresh entry instead of serving stale sources.

Cache entries are deleted automatically when:

- They've been untouched for four months.
- Once a cache exceeds 1000 entries. The least recently used entries are deleted first.

The cache is safe to delete at any time.
