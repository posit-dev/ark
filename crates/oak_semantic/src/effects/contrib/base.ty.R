# Effect stubs for base R. An `.ty.R` file is R source, a signature plus its
# `declare()` directive and no body logic, R's answer to a `.d.ts` / `.pyi`.

evalq <- function(expr, envir, enclos) {
  declare(expr = Nse("current"))
}

local <- function(expr, envir) {
  declare(expr = Nse())
}

with <- function(data, expr, ...) {
  declare(expr = Nse())
}

with.default <- function(data, expr, ...) {
  declare(expr = Nse())
}

within <- function(data, expr, ...) {
  declare(expr = Nse())
}

within.data.frame <- function(data, expr, ...) {
  declare(expr = Nse())
}

quote <- function(expr) {
  declare(expr = Quote)
}

# `library` reads `package` as the quoted symbol as written
# (`library(dplyr)` attaches `dplyr`), unless `character.only = TRUE` flips it
# to a string to evaluate. The `if` encodes that flip, matching library's own
# `character.only` branch. `character.only`'s `FALSE` default supplies the
# condition when the argument is absent.
library <- function(package, help, pos = 2, lib.loc = NULL, character.only = FALSE,
                    logical.return = FALSE, warn.conflicts, quietly = FALSE,
                    verbose = getOption("verbose"), mask.ok, exclude, include.only,
                    attach.required = missing(include.only)) {
  declare(Attach(if (.(character.only)) .(package) else .(substitute(package))))
}

require <- function(package, lib.loc = NULL, quietly = FALSE, warn.conflicts,
                    character.only = FALSE, mask.ok, exclude, include.only,
                    attach.required = missing(include.only)) {
  declare(Attach(if (.(character.only)) .(package) else .(substitute(package))))
}

# The `envir` operand resolves to the scope the sourced names land in. `local`
# picks it: `local = TRUE` sends them to the caller's scope (the call site),
# `local = FALSE` (the default) sends them to the global environment. A
# non-static `local` (e.g. an environment) leaves `envir` unresolved and drops
# the source, since its names then aren't ours to inject.
source <- function(file, local = FALSE, echo = verbose, print.eval = echo,
                   exprs, spaced = use_srcref, verbose = getOption("verbose"),
                   prompt.echo = getOption("prompt"), max.deparse.length = 150,
                   width.cutoff = 60L, deparseCtrl = "showAttributes",
                   chdir = FALSE, encoding = getOption("encoding"),
                   continue.echo = getOption("continue"), skip.echo = 0,
                   keep.source = getOption("keep.source")) {
  declare(Source(.(file), envir = if (.(local)) .(parent.frame()) else .(globalenv())))
}
