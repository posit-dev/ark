use aether_parser::parse;
use aether_parser::RParserOptions;
use aether_syntax::RSyntaxNode;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_ide::prepare_rename;
use oak_ide::rename;
use oak_ide::FileOffset;
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

fn pos(file: &Url, n: u32) -> FileOffset {
    FileOffset {
        file: file.clone(),
        offset: TextSize::from(n),
    }
}

fn edited_ranges(targets: oak_ide::RenameTargets) -> Vec<TextRange> {
    targets.ranges.into_iter().map(|r| r.range).collect()
}

// --- prepare_rename ---

#[test]
fn test_prepare_rename_on_def() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on the def `foo` at offset 0
    let result = prepare_rename(&idx, &root, &pos(&file, 0)).unwrap();
    assert_eq!(result, (text_range(0, 3), "foo".to_string()));
}

#[test]
fn test_prepare_rename_on_use() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on the use at offset 9
    let result = prepare_rename(&idx, &root, &pos(&file, 9)).unwrap();
    assert_eq!(result, (text_range(9, 12), "foo".to_string()));
}

#[test]
fn test_prepare_rename_namespace_access_returns_none() {
    let source = "dplyr::mutate\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    assert!(prepare_rename(&idx, &root, &pos(&file, 7)).is_none());
}

#[test]
fn test_prepare_rename_non_identifier_returns_none() {
    let source = "x <- 1\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    // Cursor on `<-` operator
    assert!(prepare_rename(&idx, &root, &pos(&file, 3)).is_none());
}

// --- rename: basic ---

#[test]
fn test_rename_def_and_use() {
    let source = "foo <- 1\nfoo + foo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let targets = rename(&idx, &root, &pos(&file, 0), "bar").unwrap();
    assert_eq!(targets.new_text, "bar");
    assert_eq!(edited_ranges(targets), vec![
        text_range(0, 3),
        text_range(9, 12),
        text_range(15, 18),
    ]);
}

#[test]
fn test_rename_excludes_shadowed_outer() {
    // Inner `x` is a different binding from outer `x`. Renaming inner
    // should not touch outer.
    let source = "x <- 1\nf <- function() {\n  x <- 2\n  x\n}\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let inner_def = source.find("x <- 2").unwrap() as u32;
    let targets = rename(&idx, &root, &pos(&file, inner_def), "y").unwrap();
    assert_eq!(edited_ranges(targets), vec![
        text_range(inner_def, inner_def + 1),
        text_range(
            source.rfind('x').unwrap() as u32,
            source.rfind('x').unwrap() as u32 + 1
        ),
    ]);
}

// --- rename: validation ---

#[test]
fn test_rename_empty_name_errors() {
    let source = "foo <- 1\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let err = rename(&idx, &root, &pos(&file, 0), "").unwrap_err();
    assert!(err.to_string().contains("empty"));
}

#[test]
fn test_rename_reserved_word_errors() {
    let source = "foo <- 1\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    for word in ["if", "for", "function", "TRUE", "NULL", "NA"] {
        let err = rename(&idx, &root, &pos(&file, 0), word).unwrap_err();
        assert!(
            err.to_string().contains("reserved"),
            "expected {word} to be rejected"
        );
    }
}

#[test]
fn test_rename_non_renamable_errors() {
    // Cursor on `mutate` in `dplyr::mutate` (NamespaceAccess: returns no
    // refs, so rename errors). TODO: if package is in the workspace we should
    // allow renaming.
    let source = "dplyr::mutate\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let err = rename(&idx, &root, &pos(&file, 7), "x").unwrap_err();
    assert!(err.to_string().contains("renamable") || err.to_string().contains("identifier"));
}

#[test]
fn test_rename_unbound_use_errors() {
    // Free variable: no local binding to rename.
    let source = "foo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let err = rename(&idx, &root, &pos(&file, 0), "bar").unwrap_err();
    assert!(err.to_string().contains("renamable") || err.to_string().contains("identifier"));
}

// --- rename: backtick canonicalization ---

#[test]
fn test_rename_to_name_with_space_gets_backticked() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let targets = rename(&idx, &root, &pos(&file, 0), "foo bar").unwrap();
    assert_eq!(targets.new_text, "`foo bar`");
}

#[test]
fn test_rename_to_name_with_starting_digit_gets_backticked() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let targets = rename(&idx, &root, &pos(&file, 0), "1foo").unwrap();
    assert_eq!(targets.new_text, "`1foo`");
}

#[test]
fn test_rename_to_name_with_backtick_errors() {
    let source = "foo <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let err = rename(&idx, &root, &pos(&file, 0), "foo`bar").unwrap_err();
    assert!(err.to_string().contains("backtick"));
}

// --- rename: string definitions ---

#[test]
fn test_rename_string_def_normalizes_to_identifier_form() {
    // `"foo" <- 1` defines `foo` using R's rarely-seen string-literal
    // assignment form. The semantic index records the def's range as
    // covering the whole `"foo"` token (5 chars), so rename emits an
    // edit that replaces `"foo"` with the new name in bare identifier
    // form. The user's quoting style is intentionally not preserved:
    // `bar <- 1` is valid R, more idiomatic than `"bar" <- 1`, and
    // preserving it would require per-site `new_text` plus escaping
    // rules for new names that contain quotes or backslashes.
    let source = "\"foo\" <- 1\nfoo\n";
    let file = file_url("test.R");
    let (root, idx) = parse_source(source);

    let targets = rename(&idx, &root, &pos(&file, 11), "bar").unwrap();
    assert_eq!(targets.new_text, "bar");
    assert_eq!(edited_ranges(targets), vec![
        text_range(0, 5),   // covers `"foo"` (replaced as a whole)
        text_range(11, 14), // covers `foo` (the use site)
    ]);
}
