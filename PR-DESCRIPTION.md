# Stream intermediate autoprint output in notebook mode

Addresses https://github.com/posit-dev/positron/issues/11227.

In notebook session mode, `Console::write_console` deliberately dropped autoprint output from intermediate top-level expressions, so a cell only displayed the last expression's value. R users expect every visible top-level value to print, as in the R console, RStudio, and IRkernel (the issue reporter confirmed IRkernel shows all values). This PR removes the notebook-mode suppression so intermediate autoprint falls through to the IOPub stdout stream, exactly as console mode already does.

## Before

Running this cell in a Positron notebook:

```r
1
2
3
```

only showed:

```
[1] 3
```

## After

The same cell shows all three values: `[1] 1` and `[1] 2` as stream output, and `[1] 3` as the `execute_result`. The last expression's value is not duplicated on the stream, `invisible()` intermediate expressions still produce no output, and intermediate values printed before an error are flushed (they already were via the exception path).

## Approach

The minimal fix from the options considered: reuse the existing console-mode fall-through (stream intermediate autoprint on IOPub stdout) rather than emitting per-expression `display_data` messages at expression boundaries. The stream path is already exercised by console mode (debug filter, stderr ordering); per-expression rich display can be layered on later if desired, as could an IPython-style `ast_node_interactivity` option.

## Tests

- New: `test_notebook_execute_request_intermediate_autoprint` (`1\n2\n3` streams `[1] 1\n[1] 2\n` exactly and emits `[1] 3` as the execute result, guarding against duplication) and `test_notebook_execute_request_invisible_intermediate_expression` (`invisible(1)\n2` produces no stream output).
- Updated: notebook multi-expression, error, and stdin tests now assert the streamed intermediate values.
