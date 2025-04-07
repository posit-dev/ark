Do not use `bail!`. Instead use an explicit `return Err(anyhow!(...))`.

When a `match` expression is the last expression in a function, omit `return` keywords in match arms. Let the expression evaluate to the function's return value.

For error messages and logging, prefer direct formatting syntax: `Err(anyhow!("Message: {err}"))` instead of `Err(anyhow!("Message: {}", err))`.

Use `log::trace!` instead of `log::debug!`.

Use fully qualified result types (`anyhow::Result`) instead of importing them.
