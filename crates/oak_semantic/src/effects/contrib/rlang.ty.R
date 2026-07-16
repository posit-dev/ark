# Effect stubs for rlang. `on_load` defers its expression to package load time,
# evaluating it in the calling scope. The `.(parent.frame())` operand names that
# call-site scope directly (`Current`), and `eager = FALSE` marks it lazy. The
# binding operator `%<~%` is a custom handler in `rlang.rs`.

on_load <- function(expr, env, ns) {
  declare(expr = Nse(.(parent.frame()), eager = FALSE))
}
