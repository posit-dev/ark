Do not use `bail!`. Instead use an explicit `return Err(anyhow!(...))`.

When a `match` expression is the last expression in a function, omit `return` keywords in match arms. Let the expression evaluate to the function's return value.

For error messages and logging, prefer direct formatting syntax: `Err(anyhow!("Message: {err}"))` instead of `Err(anyhow!("Message: {}", err))`. This also applies to `log::error!` and `log::warn!` and `log::info!` macros.

Use `log::trace!` instead of `log::debug!`.

Use fully qualified result types (`anyhow::Result`) instead of importing them.

When writing tests, prefer simple assertion macros without custom error messages:
- Use `assert_eq!(actual, expected);` instead of `assert_eq!(actual, expected, "custom message");`
- Use `assert!(condition);` instead of `assert!(condition, "custom message");`

Tests are run with `just test`, not `cargo test`.

When you extract code in a function (or move things around) that function goes
_below_ the calling function. A general goal is to be able to read linearly from
top to bottom with the relevant context and main logic first. The code should be
organised like a call stack. Of course that's not always possible, use best
judgement to produce the clearest code organization.

 Keep the main logic as unnested as possible. Favour Rust's `let ... else`
 syntax to return early or continue a loop in the `else` clause, over `if let`.
