use tower_lsp::lsp_types;
use url::Url;

use crate::lsp::capabilities::Capabilities;
use crate::lsp::code_action::code_action;
use crate::lsp::code_action::code_action_workspace_text_edit;
use crate::lsp::code_action::CodeActions;
use crate::lsp::documents::Document;
use crate::lsp::encoding::convert_point_to_position;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeTypeExt;

pub(crate) fn roxygen_documentation(
    actions: &mut CodeActions,
    uri: &Url,
    document: &Document,
    range: tree_sitter::Range,
    capabilities: &Capabilities,
) -> Option<()> {
    if !capabilities.code_action_literal_support() {
        // This code action returns literal `CodeAction`s, so must have support for them
        return None;
    }

    // For cursors (the common case), start and end points are the same.
    // For selections, we require that the start point be touching the function name.
    let start = range.start_point;

    let node = document
        .ast
        .root_node()
        .named_descendant_for_point_range(start, start)?;

    // User must be sitting on the function name
    if !node.is_identifier() {
        return None;
    }

    // Parent must be a `<-` or `=` assignment node
    let assignment = node.parent()?;

    if !assignment.is_binary_operator_of_kind(BinaryOperatorType::LeftAssignment) &&
        !assignment.is_binary_operator_of_kind(BinaryOperatorType::EqualsAssignment)
    {
        return None;
    }

    // And the rhs must be a function definition
    let function = assignment.child_by_field_name("rhs")?;

    if !function.is_function_definition() {
        return None;
    }

    // The assignment node must be a direct descendent of the program node (i.e. we don't
    // provide the code action for local functions defined within another function, or
    // within some arbitrary braced expression)
    if !assignment.parent()?.is_program() {
        return None;
    }

    let position = node.start_position();

    // Fairly simple detection of existing `#'` on the previous line (but starting at the
    // same `column` offset), which tells us not to provide this code action
    if let Some(previous_line) = document.contents.get_line(position.row.saturating_sub(1)) {
        if let Some(previous_line) = previous_line.get_byte_slice(position.column..) {
            let mut previous_line = previous_line.bytes();

            if previous_line
                .next()
                .map(|byte| byte == b'#')
                .unwrap_or(false) &&
                previous_line
                    .next()
                    .map(|byte| byte == b'\'')
                    .unwrap_or(false)
            {
                return None;
            }
        }
    }

    // Okay, looks like we are going to provide the code action, collect all parameter
    // names
    let parameters = function.child_by_field_name("parameters")?;

    let mut parameter_names = vec![];
    let mut cursor = parameters.walk();

    for child in parameters.children_by_field_name("parameter", &mut cursor) {
        let parameter_name = child.child_by_field_name("name")?;
        let parameter_name = document
            .contents
            .node_slice(&parameter_name)
            .ok()?
            .to_string();
        parameter_names.push(parameter_name);
    }

    let indent_size = position.column;
    let documentation = documentation_builder(parameter_names, indent_size);

    // We insert the documentation string at the start position of the function name.
    // This handles the indentation of the first documentation line, and makes new line
    // handling trivial (we just add a new line to every documentation line).
    let position = convert_point_to_position(&document.contents, position);
    let range = tower_lsp::lsp_types::Range::new(position, position);
    let edit = lsp_types::TextEdit::new(range, documentation);
    let edit =
        code_action_workspace_text_edit(uri.clone(), document.version, vec![edit], capabilities);

    actions.add_action(code_action(
        "Generate a roxygen template".to_string(),
        lsp_types::CodeActionKind::EMPTY,
        edit,
    ))
}

fn documentation_builder(parameter_names: Vec<String>, indent_size: usize) -> String {
    let mut lines = vec![];

    lines.push("#' Title".to_string());

    if !parameter_names.is_empty() {
        lines.push("#'".to_string());
        lines.append(&mut parameters_builder(parameter_names));
    }

    lines.push("#'".to_string());
    lines.push("#' @returns".to_string());
    lines.push("#'".to_string());
    lines.push("#' @export".to_string());
    lines.push("#' @examples".to_string());

    documentation_from_lines(lines, indent_size)
}

fn parameters_builder(names: Vec<String>) -> Vec<String> {
    names
        .into_iter()
        .map(|name| format!("#' @param {name}"))
        .collect()
}

/// Combine lines into a single documentation string used within a `TextEdit`
///
/// This is done in a clever way:
/// - We don't apply indentation to the first line, as our `TextEdit` will do that for us,
///   by inserting at the specified column position.
/// - We insert a newline after every line, even the last one. This allows us to cleanly
///   insert the documentation at the start position of the function name.
fn documentation_from_lines(lines: Vec<String>, indent_size: usize) -> String {
    let mut documentation = String::new();
    for line in lines {
        documentation.push_str(&line);
        documentation.push('\n');
        documentation.push_str(&" ".repeat(indent_size));
    }
    documentation
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::CodeActionOrCommand;
    use tower_lsp::lsp_types::DocumentChanges;
    use tower_lsp::lsp_types::OneOf;
    use tower_lsp::lsp_types::Position;
    use tree_sitter::Point;
    use tree_sitter::Range;
    use url::Url;

    use crate::fixtures::point_and_offset_from_cursor;
    use crate::lsp::capabilities::Capabilities;
    use crate::lsp::code_action::roxygen::roxygen_documentation;
    use crate::lsp::code_action::CodeActions;
    use crate::lsp::documents::Document;

    fn point_range(point: Point, byte: usize) -> Range {
        Range {
            start_byte: byte,
            end_byte: byte,
            start_point: point,
            end_point: point,
        }
    }

    fn roxygen_point_and_offset_from_cursor(text: &str) -> (String, Point, usize) {
        point_and_offset_from_cursor(text, b'@')
    }

    fn roxygen_documentation_test(text: &str, position: Position) -> String {
        let mut actions = CodeActions::new();

        let uri = Url::parse("file:///test.R").unwrap();

        let capabilities = Capabilities::default()
            .with_code_action_literal_support(true)
            .with_workspace_edit_document_changes(true);

        let (text, point, offset) = roxygen_point_and_offset_from_cursor(text);
        let document = Document::new(&text, None);

        roxygen_documentation(
            &mut actions,
            &uri,
            &document,
            point_range(point, offset),
            &capabilities,
        );

        let mut actions = actions.into_response();
        assert_eq!(actions.len(), 1);

        let CodeActionOrCommand::CodeAction(action) = actions.pop().unwrap() else {
            panic!("Unexpected");
        };
        let workspace_edit = action.edit.unwrap();

        let document_changes = workspace_edit.document_changes.unwrap();
        let DocumentChanges::Edits(mut text_document_edits) = document_changes else {
            panic!("Unexpected");
        };
        assert_eq!(text_document_edits.len(), 1);

        let mut text_document_edit = text_document_edits.pop().unwrap();
        assert_eq!(text_document_edit.text_document.uri, uri);
        assert_eq!(text_document_edit.edits.len(), 1);

        let OneOf::Left(text_edit) = text_document_edit.edits.pop().unwrap() else {
            panic!("Unexpected");
        };
        assert_eq!(text_edit.range.start.line, position.line);
        assert_eq!(text_edit.range.start.character, position.character);
        assert_eq!(text_edit.range.end, text_edit.range.start);

        text_edit.new_text
    }

    #[test]
    fn test_adds_parameters() {
        let new_text = roxygen_documentation_test("fu@n <- function(a, b = 2) {}", Position {
            line: 0,
            character: 0,
        });
        insta::assert_snapshot!(new_text);

        let new_text = roxygen_documentation_test("fu@n <- function(...) {}", Position {
            line: 0,
            character: 0,
        });
        insta::assert_snapshot!(new_text);

        // Mock some new lines and indentation
        // (It's correct for the first line to not be indented in the snapshot,
        // since the `Position` handles the indentation through `character`)
        let new_text = roxygen_documentation_test("\n\n    fu@n <- function(...) {}", Position {
            line: 2,
            character: 4,
        });
        insta::assert_snapshot!(new_text);
    }

    #[test]
    fn test_no_parameters() {
        let new_text = roxygen_documentation_test("fu@n <- function() {}", Position {
            line: 0,
            character: 0,
        });
        insta::assert_snapshot!(new_text);
    }

    #[test]
    fn test_supports_equals_assignment() {
        let new_text = roxygen_documentation_test("fu@n = function(a, b = 2) {}", Position {
            line: 0,
            character: 0,
        });
        insta::assert_snapshot!(new_text);
    }

    #[test]
    fn test_adds_documentation_when_direct_preceding_line_is_not_documentation() {
        let new_text = roxygen_documentation_test("#'\n\nfu@n = function(a, b = 2) {}", Position {
            line: 2,
            character: 0,
        });
        insta::assert_snapshot!(new_text);
    }

    #[test]
    fn test_no_action_when_on_local_function() {
        let mut actions = CodeActions::new();

        let uri = Url::parse("file:///test.R").unwrap();

        let capabilities = Capabilities::default()
            .with_code_action_literal_support(true)
            .with_workspace_edit_document_changes(true);

        let text = "
outer <- function(a, b = 2) {
  in@ner <- function(a, b, c) {}
}
        ";

        let (text, point, offset) = roxygen_point_and_offset_from_cursor(text);
        let document = Document::new(&text, None);

        roxygen_documentation(
            &mut actions,
            &uri,
            &document,
            point_range(point, offset),
            &capabilities,
        );
        let actions = actions.into_response();
        assert!(actions.is_empty());
    }

    #[test]
    fn test_no_action_when_documentation_on_previous_line() {
        let mut actions = CodeActions::new();

        let uri = Url::parse("file:///test.R").unwrap();

        let capabilities = Capabilities::default()
            .with_code_action_literal_support(true)
            .with_workspace_edit_document_changes(true);

        let text = "
#' Title
f@n <- function(a, b) {}
        ";

        let (text, point, offset) = roxygen_point_and_offset_from_cursor(text);
        let document = Document::new(&text, None);

        roxygen_documentation(
            &mut actions,
            &uri,
            &document,
            point_range(point, offset),
            &capabilities,
        );
        let actions = actions.into_response();
        assert!(actions.is_empty());
    }

    #[test]
    fn test_no_action_when_cursor_is_after_function_name() {
        let mut actions = CodeActions::new();

        let uri = Url::parse("file:///test.R").unwrap();

        let capabilities = Capabilities::default()
            .with_code_action_literal_support(true)
            .with_workspace_edit_document_changes(true);

        // This is just how tree-sitter works, it uses a half open range of `[)`.
        let text = "
fn@ <- function(a, b) {}
        ";

        let (text, point, offset) = roxygen_point_and_offset_from_cursor(text);
        let document = Document::new(&text, None);

        roxygen_documentation(
            &mut actions,
            &uri,
            &document,
            point_range(point, offset),
            &capabilities,
        );
        let actions = actions.into_response();
        assert!(actions.is_empty());
    }

    #[test]
    fn test_no_action_without_code_action_literal_support() {
        let mut actions = CodeActions::new();

        let uri = Url::parse("file:///test.R").unwrap();

        // NOTE: `with_code_action_literal_support(false)`
        let capabilities = Capabilities::default()
            .with_code_action_literal_support(false)
            .with_workspace_edit_document_changes(true);

        let text = "
f@n <- function(a, b) {}
        ";

        let (text, point, offset) = roxygen_point_and_offset_from_cursor(text);
        let document = Document::new(&text, None);

        roxygen_documentation(
            &mut actions,
            &uri,
            &document,
            point_range(point, offset),
            &capabilities,
        );
        let actions = actions.into_response();
        assert!(actions.is_empty());
    }

    #[test]
    fn test_uses_hash_map_of_text_edits_without_document_changes_support() {
        let mut actions = CodeActions::new();

        let uri = Url::parse("file:///test.R").unwrap();

        // NOTE: `with_workspace_edit_document_changes(false)`
        let capabilities = Capabilities::default()
            .with_code_action_literal_support(true)
            .with_workspace_edit_document_changes(false);

        let text = "
f@n <- function(a, b) {}
        ";

        let (text, point, offset) = roxygen_point_and_offset_from_cursor(text);
        let document = Document::new(&text, None);

        roxygen_documentation(
            &mut actions,
            &uri,
            &document,
            point_range(point, offset),
            &capabilities,
        );

        let mut actions = actions.into_response();
        assert_eq!(actions.len(), 1);

        let CodeActionOrCommand::CodeAction(action) = actions.pop().unwrap() else {
            panic!("Unexpected");
        };
        let workspace_edit = action.edit.unwrap();

        // This is now `None`
        assert!(workspace_edit.document_changes.is_none());

        // The edits are here instead
        let changes = workspace_edit.changes.unwrap();
        assert_eq!(changes.len(), 1);
        let text_edits = changes.get(&uri).unwrap();
        assert_eq!(text_edits.len(), 1);

        let text_edit = text_edits.get(0).unwrap();
        assert_eq!(text_edit.range.start.line, 1);
        assert_eq!(text_edit.range.start.character, 0);
        assert_eq!(text_edit.range.end, text_edit.range.start);

        insta::assert_snapshot!(text_edit.new_text);
    }
}
