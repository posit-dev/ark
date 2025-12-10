//
// console_annotate.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

use amalthea::wire::execute_request::CodeLocation;
use biome_rowan::AstNode;

pub(crate) fn annotate_input(code: &str, location: CodeLocation) -> String {
    let node = aether_parser::parse(code, Default::default()).tree();
    let Some(first_token) = node.syntax().first_token() else {
        return code.into();
    };

    let line_directive = format!(
        "#line {line} \"{uri}\"",
        line = location.start.line + 1,
        uri = location.uri
    );

    // Leading whitespace to ensure that R starts parsing expressions from
    // the expected `character` offset.
    let leading_padding = " ".repeat(location.start.character);

    // Collect existing leading trivia as (kind, text) tuples
    let existing_trivia: Vec<_> = first_token
        .leading_trivia()
        .pieces()
        .map(|piece| (piece.kind(), piece.text().to_string()))
        .collect();

    // Create new trivia with line directive prepended
    let new_trivia: Vec<_> = vec![
        (
            biome_rowan::TriviaPieceKind::SingleLineComment,
            line_directive.to_string(),
        ),
        (biome_rowan::TriviaPieceKind::Newline, "\n".to_string()),
        (
            biome_rowan::TriviaPieceKind::Whitespace,
            leading_padding.to_string(),
        ),
    ]
    .into_iter()
    .chain(existing_trivia.into_iter())
    .collect();

    let new_first_token =
        first_token.with_leading_trivia(new_trivia.iter().map(|(k, t)| (*k, t.as_str())));

    let Some(new_node) = node
        .syntax()
        .clone()
        .replace_child(first_token.into(), new_first_token.into())
    else {
        return code.into();
    };

    new_node.to_string()
}
