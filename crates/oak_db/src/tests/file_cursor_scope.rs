use biome_rowan::TextSize;
use oak_semantic::ScopeId;

use crate::test_path::file_path;
use crate::tests::test_db::TestDb;
use crate::File;
use crate::FileRevision;

/// The scope id at the `|` marker. Scope ids follow preorder: `0` is the file,
/// `1` the first nested scope, and so on.
fn scope_at_cursor(source: &str) -> ScopeId {
    let offset = source.find('|').unwrap();
    let text = source.replacen('|', "", 1);

    let db = TestDb::new();
    let file = File::new(
        &db,
        file_path("a.R"),
        FileRevision::zero(),
        Some(text),
        None,
    );
    file.cursor_scope(&db, TextSize::from(offset as u32))
}

#[test]
fn test_cursor_at_end_of_unbraced_body_is_in_function() {
    assert_eq!(
        scope_at_cursor("f <- function(x) x|\ng <- 1\n"),
        ScopeId::from(1)
    );
    assert_eq!(scope_at_cursor("f <- function(x) x|"), ScopeId::from(1));
}

#[test]
fn test_cursor_after_missing_body_is_in_function() {
    assert_eq!(scope_at_cursor("f <- function(x) |"), ScopeId::from(1));
    assert_eq!(scope_at_cursor("f <- function(x)\n|"), ScopeId::from(1));
}

#[test]
fn test_cursor_in_unclosed_braces_at_end_of_file_is_in_function() {
    assert_eq!(
        scope_at_cursor("f <- function(x) {\n  x\n  |"),
        ScopeId::from(1)
    );
    assert_eq!(scope_at_cursor("f <- function(x) {\n  |"), ScopeId::from(1));
}

#[test]
fn test_cursor_after_closing_brace_is_outside_function() {
    assert_eq!(
        scope_at_cursor("f <- function(x) { x }|\ng <- 1\n"),
        ScopeId::from(0)
    );
    assert_eq!(
        scope_at_cursor("f <- function(x) { x }\n|"),
        ScopeId::from(0)
    );
    assert_eq!(scope_at_cursor("local({\n  x <- 1\n})|"), ScopeId::from(0));
}

#[test]
fn test_cursor_after_newline_following_complete_body_is_outside_function() {
    assert_eq!(
        scope_at_cursor("f <- function(x) x\n|\ng <- 1\n"),
        ScopeId::from(0)
    );
    assert_eq!(scope_at_cursor("f <- function(x) x\n|"), ScopeId::from(0));
}

#[test]
fn test_cursor_after_newline_continues_unfinished_body() {
    assert_eq!(scope_at_cursor("f <- function(x) x +\n|"), ScopeId::from(1));
    assert_eq!(
        scope_at_cursor("lapply(xs, function(x) x\n|)"),
        ScopeId::from(1)
    );
}

#[test]
fn test_cursor_inside_token_uses_half_open_ranges() {
    assert_eq!(scope_at_cursor("f <- function(x) x|y"), ScopeId::from(1));
    assert_eq!(
        scope_at_cursor("f <- function(x) x\n|y <- 1"),
        ScopeId::from(0)
    );
}

#[test]
fn test_cursor_in_unclosed_outer_function_after_inner_function() {
    // `g` is scope 1 and `f` scope 2. The inner function's statement has
    // ended, but `g`'s braces are still open.
    assert_eq!(
        scope_at_cursor("g <- function() {\n  f <- function(x) x\n  |"),
        ScopeId::from(1)
    );
    assert_eq!(
        scope_at_cursor("g <- function() {\n  f <- function() { y }|"),
        ScopeId::from(1)
    );
}

#[test]
fn test_cursor_in_nested_function_ending_together() {
    assert_eq!(
        scope_at_cursor("f <- function(x) function(y) y|"),
        ScopeId::from(2)
    );
}

#[test]
fn test_cursor_before_any_token_is_in_file_scope() {
    assert_eq!(scope_at_cursor("|"), ScopeId::from(0));
    assert_eq!(
        scope_at_cursor("  |\nf <- function(x) x\n"),
        ScopeId::from(0)
    );
}
