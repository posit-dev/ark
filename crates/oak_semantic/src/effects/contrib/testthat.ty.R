# Effect stubs for testthat. `test_that` captures its `code` block and evaluates
# it eagerly in a nested scope (`Nse()`).

test_that <- function(desc, code) {
  declare(code = Nse())
}
