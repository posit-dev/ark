use oak_ide::ArgumentPosition;
use oak_ide::CallSite;

use crate::support::argument;
use crate::support::call_site_at_cursor;
use crate::support::range;

#[test]
fn test_arguments_before_and_after_cursor() {
    assert_eq!(
        call_site_at_cursor("f(a, b = 1, <|>, d = 2)"),
        Some(CallSite {
            callee: String::from("f"),
            callee_range: range(0, 1),
            arguments: vec![
                argument(None, Some("a"), 2, 3),
                argument(Some("b"), Some("1"), 5, 10),
                argument(None, None, 12, 12),
                argument(Some("d"), Some("2"), 14, 19),
            ],
            active: 2,
            position: ArgumentPosition::Name,
        })
    );
}

#[test]
fn test_namespaced_callee_and_named_argument() {
    assert_eq!(
        call_site_at_cursor("pkg::f(x = <|>)"),
        Some(CallSite {
            callee: String::from("pkg::f"),
            callee_range: range(0, 6),
            arguments: vec![argument(Some("x"), None, 7, 10)],
            active: 0,
            position: ArgumentPosition::Value,
        })
    );
}

#[test]
fn test_quoted_argument_names() {
    assert_eq!(
        call_site_at_cursor("f(`a` = 1, <|>)"),
        Some(CallSite {
            callee: String::from("f"),
            callee_range: range(0, 1),
            arguments: vec![
                argument(Some("a"), Some("1"), 2, 9),
                argument(None, None, 11, 11),
            ],
            active: 1,
            position: ArgumentPosition::Name,
        })
    );

    assert_eq!(
        call_site_at_cursor("f(\"a\" = 1, 'b' = 2, <|>)"),
        Some(CallSite {
            callee: String::from("f"),
            callee_range: range(0, 1),
            arguments: vec![
                argument(Some("a"), Some("1"), 2, 9),
                argument(Some("b"), Some("2"), 11, 18),
                argument(None, None, 20, 20),
            ],
            active: 2,
            position: ArgumentPosition::Name,
        })
    );

    // Callees and values retain backquotes for session evaluation.
    assert_eq!(
        call_site_at_cursor("`my f`(`my x`, <|>)"),
        Some(CallSite {
            callee: String::from("`my f`"),
            callee_range: range(0, 6),
            arguments: vec![
                argument(None, Some("`my x`"), 7, 13),
                argument(None, None, 15, 15),
            ],
            active: 1,
            position: ArgumentPosition::Name,
        })
    );
}

#[test]
fn test_cursor_before_a_comma_is_in_the_argument_before_it() {
    assert_eq!(
        call_site_at_cursor("f(x = <|>, y)"),
        Some(CallSite {
            callee: String::from("f"),
            callee_range: range(0, 1),
            arguments: vec![
                argument(Some("x"), None, 2, 5),
                argument(None, Some("y"), 8, 9),
            ],
            active: 0,
            position: ArgumentPosition::Value,
        })
    );
}

#[test]
fn test_value_after_cursor_leaves_a_name_position() {
    // Before an existing value, the user can still insert an argument name.
    assert_eq!(
        call_site_at_cursor("f(x = 1, <|>2)"),
        Some(CallSite {
            callee: String::from("f"),
            callee_range: range(0, 1),
            arguments: vec![
                argument(Some("x"), Some("1"), 2, 7),
                argument(None, Some("2"), 9, 10),
            ],
            active: 1,
            position: ArgumentPosition::Name,
        })
    );
}

#[test]
fn test_value_positions() {
    let position = |code| call_site_at_cursor(code).map(|call| call.position);

    assert_eq!(position("match(x =<|>)"), Some(ArgumentPosition::Value));
    assert_eq!(
        position("match(\n  x = <|>\n)"),
        Some(ArgumentPosition::Value)
    );
    assert_eq!(position("f(x + <|>)"), Some(ArgumentPosition::Value));
    assert_eq!(position("f(x <|>)"), Some(ArgumentPosition::Value));
    assert_eq!(position("f(ab<|> = 1)"), Some(ArgumentPosition::Name));
}

#[test]
fn test_outside_parentheses() {
    assert_eq!(call_site_at_cursor("f<|>()"), None);
    assert_eq!(call_site_at_cursor("f()<|>"), None);
    assert_eq!(call_site_at_cursor("x<|>"), None);
}

fn active_formal(code: &str) -> Option<usize> {
    let formals = ["a", "b", "c", "d"];
    call_site_at_cursor(code).unwrap().active_formal(&formals)
}

#[test]
fn test_active_formal() {
    assert_eq!(active_formal("f(<|>)"), Some(0));
    assert_eq!(active_formal("f(1, <|>)"), Some(1));

    // Named arguments reserve their formals even when supplied after the cursor.
    assert_eq!(active_formal("f(1, <|>, b = 2)"), Some(2));
    assert_eq!(active_formal("f(a, b = 1, <|>, d = 2)"), Some(2));

    assert_eq!(active_formal("f(1, c = <|>)"), Some(2));
    assert_eq!(active_formal("f(d = 1<|>)"), Some(3));

    assert_eq!(active_formal("f(1, 2, 3, 4, <|>)"), None);

    // An unknown name falls back to positional matching.
    assert_eq!(active_formal("f(zzz = <|>)"), Some(0));
}
