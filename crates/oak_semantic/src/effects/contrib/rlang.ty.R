# Effect stubs for rlang. `on_load` defers its expression to package load time,
# evaluating it in the calling scope (`Nse("current", eager = FALSE)`). The
# binding operator `%<~%` is a custom handler in `rlang.rs`.

on_load <- function(expr, env, ns) {
  declare(expr = Nse("current", eager = FALSE))
}
