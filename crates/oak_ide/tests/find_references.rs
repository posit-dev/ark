use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::RSyntaxNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_ide::find_references;
use oak_ide::FilePosition;
use oak_ide::FileRange;
use oak_semantic::build_index;
use oak_semantic::semantic_index::SemanticIndex;
use oak_semantic::NoopImportsResolver;
use url::Url;

fn parse_source(source: &str) -> (RSyntaxNode, SemanticIndex) {
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let index = build_index(&parsed.tree(), NoopImportsResolver);
    (root, index)
}

fn text_range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end))
}

fn file_url(name: &str) -> Url {
    Url::parse(&format!("file:///project/R/{name}")).unwrap()
}

fn offset(n: u32) -> TextSize {
    TextSize::from(n)
}

fn pos(file: &Url, n: u32) -> FilePosition {
    FilePosition {
        file: file.clone(),
        offset: offset(n),
    }
}

fn ranges(refs: Vec<FileRange>) -> Vec<TextRange> {
    refs.into_iter().map(|r| r.range).collect()
}

// --- Local resolution ---

#[test]
fn test_local_simple() {
    // "x <- 1\nx\n"
    //  0123456 78
    let source = "x <- 1\nx\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on the use at offset 7
    let refs = ranges(find_references(&idx, &root, &pos(&file, 7), true));
    assert_eq!(refs, vec![text_range(0, 1), text_range(7, 8)]);
}

#[test]
fn test_local_excludes_declaration() {
    let source = "x <- 1\nx\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // With include_declaration = false, the def at 0..1 is dropped
    let refs = ranges(find_references(&idx, &root, &pos(&file, 7), false));
    assert_eq!(refs, vec![text_range(7, 8)]);
}

#[test]
fn test_from_definition_site() {
    // Cursor on the def `x` in `x <- 1` should still return all refs
    let source = "x <- 1\nx\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let refs = ranges(find_references(&idx, &root, &pos(&file, 0), true));
    assert_eq!(refs, vec![text_range(0, 1), text_range(7, 8)]);
}

#[test]
fn test_shadowing_excludes_outer() {
    // Outer `x` and inner `x` are different bindings; refs of inner
    // shouldn't include outer (and vice versa).
    //
    // "x <- 1\nf <- function() {\n  x <- 2\n  x\n}\n"
    //  0      7                  26       35
    //  outer def at 0..1, outer never used
    //  inner def at 28..29, inner use at 37..38
    let source = "x <- 1\nf <- function() {\n  x <- 2\n  x\n}\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on the inner use `x` (offset 37). Should return inner pair only.
    let inner_def = source.find("x <- 2").unwrap() as u32;
    let inner_use = source.rfind('x').unwrap() as u32;
    let refs = ranges(find_references(&idx, &root, &pos(&file, inner_use), true));
    assert_eq!(refs, vec![
        text_range(inner_def, inner_def + 1),
        text_range(inner_use, inner_use + 1),
    ]);

    // Cursor on the outer def `x` (offset 0). Outer has no uses, so just
    // the def itself.
    let refs = ranges(find_references(&idx, &root, &pos(&file, 0), true));
    assert_eq!(refs, vec![text_range(0, 1)]);
}

#[test]
fn test_free_variable_in_inner_scope() {
    // Free `x` inside `f` resolves to file-scope `x`. Refs should include
    // the file-scope def and the inner use.
    let source = "x <- 1\nf <- function() {\n  x\n}\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let inner_use = source.rfind('x').unwrap() as u32;
    let refs = ranges(find_references(&idx, &root, &pos(&file, inner_use), true));
    assert_eq!(refs, vec![
        text_range(0, 1),
        text_range(inner_use, inner_use + 1)
    ]);
}

#[test]
fn test_multiple_uses() {
    let source = "x <- 1\nx + x + x\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let refs = ranges(find_references(&idx, &root, &pos(&file, 0), true));
    assert_eq!(refs, vec![
        text_range(0, 1),
        text_range(7, 8),
        text_range(11, 12),
        text_range(15, 16),
    ]);
}

#[test]
fn test_parameter_refs() {
    let source = "f <- function(x) {\n  x + x\n}\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on parameter `x` at offset 14
    let refs = ranges(find_references(&idx, &root, &pos(&file, 14), true));
    assert_eq!(refs.len(), 3);
    assert_eq!(refs[0], text_range(14, 15));
}

#[test]
fn test_reassignment_separates_refs() {
    // After `x <- 2`, the use of `x` binds to the second def. Find-refs on
    // the second def returns just the def itself and the use that follows.
    let source = "x <- 1\nx <- 2\nx\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on second def `x` at offset 7
    let refs = ranges(find_references(&idx, &root, &pos(&file, 7), true));
    // Use at offset 14 binds to the second def. The first def at offset 0
    // is killed by the second.
    assert_eq!(refs, vec![text_range(7, 8), text_range(14, 15)]);

    // Cursor on first def at offset 0. The only ref is the def itself
    // (its use is shadowed by the second def).
    let refs = ranges(find_references(&idx, &root, &pos(&file, 0), true));
    assert_eq!(refs, vec![text_range(0, 1)]);
}

#[test]
fn test_conditional_defs_seen_from_enclosing_scope() {
    // Outer scope has conditional defs; inner function uses the name as
    // a free variable. The use's target def set has BOTH outer defs
    // (multiple-element set via the enclosing snapshot). Refs should
    // include both defs and the inner use.
    //
    //  "if (TRUE) x <- 1 else x <- 2\nf <- function() x\n"
    //   0         10       16     22                 45
    let source = "if (TRUE) x <- 1 else x <- 2\nf <- function() x\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let use_offset = source.rfind('x').unwrap() as u32;
    let refs = ranges(find_references(&idx, &root, &pos(&file, use_offset), true));
    assert_eq!(refs, vec![
        text_range(10, 11),
        text_range(22, 23),
        text_range(use_offset, use_offset + 1),
    ]);
}

#[test]
fn test_super_assignment_targets_outer_scope() {
    // `x <<- 2` inside `f` defines `x` in the enclosing scope. A use of
    // `x` outside `f` sees BOTH the original def and the super-assigned
    // def (the use-def map adds `<<-` without clearing).
    //
    //  "x <- 1\nf <- function() x <<- 2\nx\n"
    //   0                       23      31
    let source = "x <- 1\nf <- function() x <<- 2\nx\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let super_def = source.find("x <<-").unwrap() as u32;
    let outer_use = source.rfind('x').unwrap() as u32;
    let refs = ranges(find_references(&idx, &root, &pos(&file, outer_use), true));
    // Both file-scope defs (`x <- 1` and the super-assigned `x`) and the
    // use are returned.
    assert_eq!(refs, vec![
        text_range(0, 1),
        text_range(super_def, super_def + 1),
        text_range(outer_use, outer_use + 1),
    ]);
}

#[test]
fn test_conditional_binding_includes_both_defs() {
    let source = "if (TRUE) x <- 1 else x <- 2\nx\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let use_offset = source.rfind('x').unwrap() as u32;
    let refs = ranges(find_references(&idx, &root, &pos(&file, use_offset), true));
    // Both conditional defs reach the use, so both are returned.
    assert_eq!(refs, vec![
        text_range(10, 11),
        text_range(22, 23),
        text_range(use_offset, use_offset + 1),
    ]);
}

// --- No resolution ---

#[test]
fn test_no_identifier_at_offset() {
    let source = "x <- 1\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on `<-` operator
    let refs = find_references(&idx, &root, &pos(&file, 3), true);
    assert!(refs.is_empty());
}

#[test]
fn test_unbound_use_returns_empty() {
    // `foo` has no local def: classification gives Use but the target def
    // set is empty. Within-file find-refs returns nothing; cross-file
    // textual fallback in the ark wrapper handles this case.
    let source = "foo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let refs = find_references(&idx, &root, &pos(&file, 0), true);
    assert!(refs.is_empty());
}

#[test]
fn test_fixme_namespace_access_returns_empty() {
    let source = "dplyr::mutate\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on `mutate` - NamespaceAccess, returns empty.
    let refs = find_references(&idx, &root, &pos(&file, 7), true);
    assert!(refs.is_empty());
}

// --- Dollar/at member access (places TODO) ---

#[test]
fn test_fixme_dollar_lhs_resolves_only_to_variable() {
    // `foo` on the LHS of `$` is a real variable use. Find-refs should
    // include the def and the LHS use, but NOT the RHS member name.
    let source = "foo <- list()\nfoo$foo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on `foo` LHS at offset 14
    let refs = ranges(find_references(&idx, &root, &pos(&file, 14), true));
    // Def at 0..3, LHS use at 14..17. RHS `foo` (offset 18..21) is a
    // member name, not tracked by the semantic index.
    assert_eq!(refs, vec![text_range(0, 3), text_range(14, 17)]);
}

#[test]
fn test_fixme_dollar_rhs_returns_empty() {
    // Cursor on `bar` in `foo$bar` - member names aren't variable refs.
    let source = "foo <- list()\nfoo$bar\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let refs = find_references(&idx, &root, &pos(&file, 18), true);
    assert!(refs.is_empty());
}

#[test]
fn test_string_def_returns_quoted_range_for_def() {
    // `"foo" <- 1` defines `foo`. The semantic index records the def's
    // range as covering the whole `"foo"` token (5 chars). Find-refs on
    // the use of `foo` returns the def at the quoted range and uses at
    // the bare range -- semantically correct, just visually unusual.
    let source = "\"foo\" <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on the bare `foo` use at offset 11
    let refs = ranges(find_references(&idx, &root, &pos(&file, 11), true));
    assert_eq!(refs, vec![
        text_range(0, 5),   // covers `"foo"` (the def site)
        text_range(11, 14), // covers `foo` (the use site)
    ]);
}
