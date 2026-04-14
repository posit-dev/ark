//
// definitions.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use anyhow::Result;
use oak_ide::ResolvedDefinition;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use tower_lsp::lsp_types::LocationLink;

use crate::lsp::document::Document;
use crate::lsp::state::WorldState;

pub(crate) fn goto_definition(
    document: &Document,
    params: GotoDefinitionParams,
    state: &WorldState,
) -> Result<Option<GotoDefinitionResponse>> {
    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let offset = from_proto::offset_from_position(
        position,
        &document.line_index,
        document.position_encoding,
    )?;

    let index = document.semantic_index();

    let Some(resolved) =
        oak_ide::goto_definition(&index, &state.root_scope(), &state.library, offset)
    else {
        return Ok(None);
    };

    match resolved {
        ResolvedDefinition::Local { range } => {
            let lsp_range =
                to_proto::range(range, &document.line_index, document.position_encoding)?;
            let link = LocationLink {
                origin_selection_range: None,
                target_uri: uri,
                target_range: lsp_range,
                target_selection_range: lsp_range,
            };
            Ok(Some(GotoDefinitionResponse::Link(vec![link])))
        },

        ResolvedDefinition::ProjectFile {
            file,
            name: _,
            range,
        } => {
            let Some(target_doc) = state.documents.get(&file) else {
                return Ok(None);
            };
            let lsp_range =
                to_proto::range(range, &target_doc.line_index, target_doc.position_encoding)?;
            let link = LocationLink {
                origin_selection_range: None,
                target_uri: file,
                target_range: lsp_range,
                target_selection_range: lsp_range,
            };
            Ok(Some(GotoDefinitionResponse::Link(vec![link])))
        },

        ResolvedDefinition::Package { .. } => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;
    use tower_lsp::lsp_types;

    use super::*;
    use crate::lsp::document::Document;
    use crate::lsp::util::test_path;

    fn make_params(uri: lsp_types::Url, line: u32, character: u32) -> GotoDefinitionParams {
        GotoDefinitionParams {
            text_document_position_params: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri },
                position: lsp_types::Position::new(line, character),
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    #[test]
    fn test_goto_definition() {
        let code = "foo <- 42\nprint(foo)\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");

        let params = make_params(uri, 1, 6);

        assert_matches!(
            goto_definition(&doc, params, &WorldState::default()).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 3),
                    }
                );
            }
        );
    }

    #[test]
    fn test_goto_definition_prefers_local_symbol() {
        let code = "foo <- 1\nfoo\n";
        let doc = Document::new(code, None);
        let uri = test_path("file.R");

        let params = make_params(uri.clone(), 1, 0);

        assert_matches!(
            goto_definition(&doc, params, &WorldState::default()).unwrap(),
            Some(GotoDefinitionResponse::Link(ref links)) => {
                assert_eq!(links[0].target_uri, uri);
                assert_eq!(
                    links[0].target_range,
                    lsp_types::Range {
                        start: lsp_types::Position::new(0, 0),
                        end: lsp_types::Position::new(0, 3),
                    }
                );
            }
        );
    }

    #[test]
    fn test_goto_definition_no_use_returns_none() {
        let code = "x <- 1\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");

        // Cursor on the `<-` operator, not a use site
        let params = make_params(uri, 0, 3);
        let result = goto_definition(&doc, params, &WorldState::default()).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_goto_definition_unresolved_returns_none() {
        let code = "foo\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");

        let params = make_params(uri, 0, 0);
        let result = goto_definition(&doc, params, &WorldState::default()).unwrap();
        assert_eq!(result, None);
    }
}
