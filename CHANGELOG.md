# Changelog

# 0.1.9000

## 2024-11

- LSP: Assignments in function calls (e.g. `list(x <- 1)`) are now detected by the missing symbol linter to avoid annoying false positive diagnostics (https://github.com/posit-dev/positron/issues/3048). The downside is that this causes false negatives when the assignment happens in a call with local scope, e.g. in `local()` or `test_that()`. We prefer to be overly permissive than overly cautious in these matters.

- Jupyter: The following environment variables are now set in the same way that R does:

  - `R_SHARE_DIR`
  - `R_INCLUDE_DIR`
  - `R_DOC_DIR`

  This solves a number of problems in situations that depend on these variables being defined (https://github.com/posit-dev/positron/issues/3637).

## 2024-10

- Objects assigned at top level are now indexed, in addition to assigned functions. When a name is assigned multiple times, we now only index the first occurrence. This allows you to jump to the first "declaration" of the variable. In the future we'll improve this mechanism so that you can jump to the most recent assignment.

  We also index `method(generic, class) <-` assignment to help with S7 development. This might be replaced by a "Find implementations" mechanism in the future.

- Results from completions have been improved with extra details.
  Package functions now display the package name (posit-dev/positron#5225)
  and namespace completions now display `::` to hint at what is being
  completed.

- The document symbol kind for assigned variables is now `VARIABLE` (@kv9898, posit-dev/positron#5071). This produces a clearer icon in the outline.

- Added support for outline headers in comments (@kv9898, posit-dev/positron#3822).

- Sending long inputs of more than 4096 bytes no longer fails (posit-dev/positron#4745).

- Jupyter: Fixed a bug in the kernel-info reply where the `pygments_lexer` field
  would be set incorrectly to `""` (#553).

  Following this fix, syntax highlighting now works correctly in Jupyter applications.


- Start of changelog.
