# Effect stubs for shiny. Every reactive-building call captures its expression
# and evaluates it lazily in a nested scope (`Nse(eager = FALSE)`).

observe <- function(x) {
  declare(x = Nse(eager = FALSE))
}

reactive <- function(x) {
  declare(x = Nse(eager = FALSE))
}

renderPlot <- function(expr, ...) {
  declare(expr = Nse(eager = FALSE))
}

renderPrint <- function(expr, ...) {
  declare(expr = Nse(eager = FALSE))
}

renderTable <- function(expr, ...) {
  declare(expr = Nse(eager = FALSE))
}

renderText <- function(expr, ...) {
  declare(expr = Nse(eager = FALSE))
}

renderUI <- function(expr, ...) {
  declare(expr = Nse(eager = FALSE))
}
