use std::path::PathBuf;

use aether_lsp_utils::proto::PositionEncoding;
use aether_path::FilePath;
use biome_line_index::WideEncoding;
use harp::eval::RParseEvalOptions;
use harp::RFunction;
use oak_db::File;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::Uri;
use url::Url;

use crate::console::console_scopes;
use crate::lsp::diagnostics;
use crate::lsp::diagnostics::DiagnosticsConfig;
use crate::lsp::harness::LspHarness;
use crate::r_task;

/// A snapshot must serve documents opened by the live state, as
/// `DiagnosticsState::refresh_all()` requires when it sends work to a worker.
#[test]
fn test_diagnostics_published_through_refresh_snapshot() {
    let url = Url::parse("file:///test.R").unwrap();
    let path = FilePath::from_url(&url);

    // `r_task()` must enclose the harness because Salsa ingredients cannot cross
    // the task's thread boundary.
    let diagnostics = r_task(|| {
        let mut harness = LspHarness::new(OakDatabase::new());
        harness
            .prepare_document(&path, String::from("foo"))
            .unwrap();

        let snapshot = harness.snapshot();
        harness.diagnose(&path, snapshot).unwrap()
    });

    assert!(!diagnostics.is_empty());
}

#[derive(Default)]
struct TestState {
    db: OakDatabase,
    session_scopes: Vec<Vec<String>>,
    config: DiagnosticsConfig,
}

impl TestState {
    fn with_db(db: OakDatabase) -> Self {
        Self {
            db,
            ..Default::default()
        }
    }

    fn diagnose(&self, file: File, uri: &Uri) -> Vec<lsp_types::Diagnostic> {
        diagnostics::generate_diagnostics(
            &self.db,
            file,
            uri,
            &self.config,
            PositionEncoding::Wide(WideEncoding::Utf16),
            &self.session_scopes,
            false,
        )
    }
}

fn generate_diagnostics(code: &str, state: &TestState) -> Vec<lsp_types::Diagnostic> {
    let url = url::Url::parse("file:///test.R").unwrap();
    let uri: Uri = url.as_str().parse().unwrap();
    let file = oak_db::File::new(
        &state.db,
        FilePath::from_url(&url),
        oak_db::FileRevision::zero(),
        Some(code.to_string()),
        None,
    );
    state.diagnose(file, &uri)
}

/// Use the test R session's library paths and console scopes so diagnostics
/// recognize its installed packages and global and search-path symbols.
fn current_state() -> TestState {
    let library_paths: Vec<String> = RFunction::new("base", ".libPaths")
        .call()
        .unwrap()
        .try_into()
        .unwrap();
    let library_paths: Vec<PathBuf> = library_paths.into_iter().map(PathBuf::from).collect();

    let mut db = OakDatabase::new();
    db.set_library_paths(&library_paths);

    let mut state = TestState::with_db(db);
    state.session_scopes = console_scopes();
    state
}

#[test]
fn test_mixed_syntax_and_semantic_diagnostics() {
    r_task(|| {
        // - `foo` is an unknown symbol
        // - `}` is a syntax error, but localized to that line
        let text = "
foo

1 }";
        let diagnostics = generate_diagnostics(text, &current_state());
        assert_eq!(diagnostics.len(), 2);

        let diagnostic = diagnostics.first().unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(3, 2));
        assert_eq!(diagnostic.range.end, Position::new(3, 3));

        let diagnostic = diagnostics.get(1).unwrap();
        insta::assert_snapshot!(diagnostic.message);
        assert_eq!(diagnostic.range.start, Position::new(1, 0));
        assert_eq!(diagnostic.range.end, Position::new(1, 3));
    })
}

#[test]
fn test_comment_after_call_argument() {
    r_task(|| {
        let text = "
            match(
                1,
                2 # hi there
            )";
        let diagnostics = generate_diagnostics(text, &current_state());
        assert!(diagnostics.is_empty());
    })
}

#[test]
fn test_missing_namespace_rhs() {
    r_task(|| {
        let text = "base::";
        let diagnostics = generate_diagnostics(text, &current_state());
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = diagnostics.first().unwrap();
        insta::assert_snapshot!(diagnostic.message);
    })
}

#[test]
fn test_no_diagnostic_for_dot_dot_i() {
    r_task(|| {
        let text = "..1 + ..2 + 3";

        let diagnostics = generate_diagnostics(text, &current_state());

        assert!(diagnostics.is_empty());
    })
}

#[test]
fn test_no_diagnostic_for_rhs_of_extractor() {
    r_task(|| {
        let options = RParseEvalOptions {
            forbid_function_calls: false,
            ..Default::default()
        };

        // Put the LHS in scope
        harp::parse_eval("x <- NULL", options.clone()).unwrap();
        let state = current_state();

        let text = "x$foo";
        let diagnostics = generate_diagnostics(text, &state);
        assert!(diagnostics.is_empty());

        let text = "x@foo";
        let diagnostics = generate_diagnostics(text, &state);
        assert!(diagnostics.is_empty());

        // Clean up
        harp::parse_eval("remove(x)", options.clone()).unwrap();
    })
}

#[test]
fn test_no_diagnostic_for_assignment_bindings() {
    r_task(|| {
        let text = "
                x <- 1
                2 -> y
                z = 3
                y + x + z
            ";
        let diagnostics = generate_diagnostics(text, &current_state());
        assert!(diagnostics.is_empty());
    })
}

#[test]
fn test_no_diagnostic_for_super_assignment_bindings() {
    r_task(|| {
        let text = "
                x <<- 1
                2 ->> y
                y + x
            ";
        let diagnostics = generate_diagnostics(text, &current_state());
        assert!(diagnostics.is_empty());
    })
}

#[test]
fn test_symbol_not_in_scope_diagnostic_is_ordering_dependent() {
    r_task(|| {
        let text = "
                x + 1
                x <- 1
                x + 1
            ";

        let diagnostics = generate_diagnostics(text, &current_state());
        assert_eq!(diagnostics.len(), 1);

        // Only marks the `x` before the `x <- 1`
        let diagnostic = diagnostics.first().unwrap();
        assert_eq!(diagnostic.range.start.line, 1)
    })
}

#[test]
fn test_no_diagnostic_formula() {
    r_task(|| {
        let text = "
                foo ~ bar
                ~foo
                identity(foo ~ bar)
                identity(~foo)
            ";
        let diagnostics = generate_diagnostics(text, &current_state());
        assert!(diagnostics.is_empty());
    })
}

#[test]
fn test_dotty_assignment_basic() {
    r_task(|| {
        let code = "
                .[apple, banana] <- c(1, 2)
                apple
                banana
                cherry
            ";

        let diagnostics = generate_diagnostics(code, &current_state());
        assert_eq!(diagnostics.len(), 1);

        let diagnostic = diagnostics.first().unwrap();
        insta::assert_snapshot!(diagnostic.message);
    })
}

#[test]
fn test_dotty_right_assignment_basic() {
    r_task(|| {
        let code = "
                c(1, 2) -> .[apple, banana]
                apple
                banana
                cherry
            ";

        let diagnostics = generate_diagnostics(code, &current_state());
        assert_eq!(diagnostics.len(), 1);

        let diagnostic = diagnostics.first().unwrap();
        insta::assert_snapshot!(diagnostic.message);
    })
}

#[test]
fn test_dotty_assignment_named() {
    r_task(|| {
        // `x` should not be defined
        let code = "
                .[apple = x, banana] <- list(w = 1, x = 2, y = 3, z = 4)
                apple
                banana
                x
            ";

        let diagnostics = generate_diagnostics(code, &current_state());
        assert_eq!(diagnostics.len(), 1);

        let diagnostic = diagnostics.first().unwrap();
        insta::assert_snapshot!(diagnostic.message);
    })
}

#[test]
fn test_dotty_assignment_recursive() {
    r_task(|| {
        let code = "
                .[apple, .[banana]] <- list(1, list(2))
                apple
                banana
                cherry
            ";

        let diagnostics = generate_diagnostics(code, &current_state());
        assert_eq!(diagnostics.len(), 1);

        let diagnostic = diagnostics.first().unwrap();
        insta::assert_snapshot!(diagnostic.message);
    })
}

#[test]
fn test_dotty_assignment_within_magrittr_pipe_braced_expr() {
    r_task(|| {
        let code = "
                mtcars %>% list({ .[apple] <- 1; apple })
                apple
            ";

        let diagnostics = generate_diagnostics(code, &current_state());
        assert_eq!(diagnostics.len(), 0);
    })
}

#[test]
fn test_dotty_assignment_within_native_pipe_braced_expr() {
    r_task(|| {
        let code = "
                mtcars |> list({ .[apple] <- 1; apple })
                apple
            ";

        let diagnostics = generate_diagnostics(code, &current_state());
        assert_eq!(diagnostics.len(), 0);
    })
}

#[test]
fn test_assignment_within_call_like() {
    // https://github.com/posit-dev/positron/issues/3048
    // With our current approach we also incorrectly index symbols in calls
    // with local scopes such as `local()` or `test_that()`. We prefer to be
    // overly permissive than the opposite to avoid annoying false positive
    // diagnostics.

    // Calls
    r_task(|| {
        let code = "
                list(x <- 1)
                x
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        let code = "
                list({ x <- 1 })
                x
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);
    });

    // Subset
    r_task(|| {
        let code = "
                foo <- list()
                foo[x <- 1]
                x
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        let code = "
                foo <- list()
                foo[{x <- 1}]
                x
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);
    });

    // Subset2
    r_task(|| {
        let code = "
                foo <- list()
                foo[[x <- 1]]
                x
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        let code = "
                foo <- list()
                foo[[{x <- 1}]]
                x
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);
    });
}

#[test]
fn test_no_symbol_diagnostics_in_call_like() {
    // For now we never check for missing symbols inside call-like nodes because we
    // don't have a good way to deal with NSE in functions like `quote()` or
    // `mutate()` or data.table's `[`.

    // Calls
    r_task(|| {
        let code = "
                list(x)
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        // Important to test nested case. We have a dynamic stack of state
        // variable to keep track of whether we are in a call. The inner
        // call should restore the outer state on exit.
        let code = "
                list(list(), x)
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        // `in_call_like_arguments` state variable is reset
        let code = "
                list()
                x
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 1);
    });

    // Subset
    r_task(|| {
        // Imagine this is `data.table()` (we don't necessarily have the package
        // installed in the test)
        // https://github.com/posit-dev/positron/issues/5271
        let code = "
                data <- data.frame(x = 1)
                data[x]
                data[,x]
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        // Imagine this is `data.table()` (we don't necessarily have the package
        // installed in the test)
        // https://github.com/posit-dev/positron/issues/3749
        let code = "
                data <- data.frame(x = 1)
                data[, y := x + 1]
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);
    });

    // Subset2
    r_task(|| {
        let code = "
                foo <- list()
                foo[[x]]
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);
    });
}

#[test]
fn test_no_symbol_diagnostics_for_native_pipe_placeholder() {
    r_task(|| {
        // https://github.com/posit-dev/positron/issues/4102
        let code = "
                x <- list(a = 1)
                x |> _$a[1]
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        // Imagine this is a data.table
        // https://github.com/posit-dev/positron/issues/3749
        let code = "
                data <- data.frame(a = 1)
                data |> _[1]
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);

        // We technically disable diagnostics for this symbol everywhere, even outside
        // of pipe scope, which is probably fine
        let code = "
                _
            ";
        assert_eq!(generate_diagnostics(code, &current_state()).len(), 0);
    })
}

#[test]
fn test_oak_diagnostics_ambiguous_callee_resolution_experimental() {
    r_task(|| {
        // `local`'s NSE reading could be shadowed by the later
        // `local <- identity` at file scope, with undetermined timing
        // relative to `f`'s call.
        let text = "f <- function() local({ x <- 1 })\nlocal <- identity\n";

        let mut state = current_state();
        state.config.experimental = true;
        let diagnostics = generate_diagnostics(text, &state);

        // This snippet doesn't trip any of the legacy tree-sitter checks:
        // `local` and `identity` both resolve to base R.
        assert_eq!(diagnostics.len(), 1);
        let diagnostic = &diagnostics[0];

        assert_eq!(
            diagnostic.code,
            Some(lsp_types::NumberOrString::String(
                "ambiguous-callee-resolution".to_string()
            ))
        );
        assert_eq!(diagnostic.range.start, Position::new(0, 16));
        assert_eq!(diagnostic.range.end, Position::new(0, 33));
        assert_eq!(diagnostic.source, Some("ark".to_string()));
        assert_eq!(
            diagnostic.severity,
            Some(lsp_types::DiagnosticSeverity::INFORMATION)
        );

        let related = diagnostic.related_information.as_ref().unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].message, "could run before the call");
        assert_eq!(related[0].location.range.start, Position::new(1, 0));
        assert_eq!(related[0].location.range.end, Position::new(1, 5));
    })
}

#[test]
fn test_oak_diagnostics_ambiguous_callee_resolution_disabled_by_default() {
    r_task(|| {
        let text = "f <- function() local({ x <- 1 })\nlocal <- identity\n";

        let diagnostics = generate_diagnostics(text, &current_state());

        assert!(diagnostics.is_empty());
    })
}
