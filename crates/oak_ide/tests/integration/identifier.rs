use aether_parser::parse;
use aether_parser::RParserOptions;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_ide::Identifier;
use oak_semantic::build_index;
use oak_semantic::NoopImportsResolver;

fn text_range(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end))
}

fn offset(n: u32) -> TextSize {
    TextSize::from(n)
}

#[test]
fn test_namespace_classify() {
    let source = "dplyr::mutate\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = build_index(&parsed.tree(), NoopImportsResolver);

    // Cursor on `mutate` (offset 7)
    let ident = Identifier::classify(&idx, &root, offset(7));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "dplyr".to_string(),
            symbol: "mutate".to_string(),
            internal: false,
            package_range: text_range(0, 5),
            symbol_range: text_range(7, 13),
        })
    );

    // Cursor on `dplyr` (offset 2)
    let ident = Identifier::classify(&idx, &root, offset(2));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "dplyr".to_string(),
            symbol: "mutate".to_string(),
            internal: false,
            package_range: text_range(0, 5),
            symbol_range: text_range(7, 13),
        })
    );
}

#[test]
fn test_namespace_classify_triple_colon() {
    let source = "pkg:::sym\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = build_index(&parsed.tree(), NoopImportsResolver);

    let ident = Identifier::classify(&idx, &root, offset(6));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "pkg".to_string(),
            symbol: "sym".to_string(),
            internal: true,
            package_range: text_range(0, 3),
            symbol_range: text_range(6, 9),
        })
    );
}

#[test]
fn test_namespace_classify_in_call() {
    // foo::bar()
    // 0123456789
    let source = "foo::bar()\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = build_index(&parsed.tree(), NoopImportsResolver);

    let ident = Identifier::classify(&idx, &root, offset(5));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "foo".to_string(),
            symbol: "bar".to_string(),
            internal: false,
            package_range: text_range(0, 3),
            symbol_range: text_range(5, 8),
        })
    );
}

#[test]
fn test_namespace_classify_in_extract() {
    // foo::bar$baz
    // 0123456789...
    let source = "foo::bar$baz\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = build_index(&parsed.tree(), NoopImportsResolver);

    // Cursor on `bar` (offset 5) -- inside the RNamespaceExpression
    let ident = Identifier::classify(&idx, &root, offset(5));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "foo".to_string(),
            symbol: "bar".to_string(),
            internal: false,
            package_range: text_range(0, 3),
            symbol_range: text_range(5, 8),
        })
    );

    // Cursor on `baz` (offset 9) -- RHS of $, not a namespace access
    let ident = Identifier::classify(&idx, &root, offset(9));
    assert_eq!(ident, None);
}

#[test]
fn test_namespace_classify_string_selectors() {
    // "foo"::"bar"
    //  0123456789...
    let source = "\"foo\"::\"bar\"\n";
    let parsed = parse(source, RParserOptions::default());
    let root = parsed.syntax();
    let idx = build_index(&parsed.tree(), NoopImportsResolver);

    let ident = Identifier::classify(&idx, &root, offset(7));
    assert_eq!(
        ident,
        Some(Identifier::NamespaceAccess {
            package: "foo".to_string(),
            symbol: "bar".to_string(),
            internal: false,
            package_range: text_range(0, 5),
            symbol_range: text_range(7, 12),
        })
    );
}
