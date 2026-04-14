//
// definitions.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::to_proto;
use oak_ide::NavigationTarget;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use tower_lsp::lsp_types::LocationLink;

use crate::lsp::document::Document;
use crate::lsp::state::WorldState;

pub(crate) fn goto_definition(
    document: &Document,
    params: GotoDefinitionParams,
    state: &WorldState,
) -> anyhow::Result<Option<GotoDefinitionResponse>> {
    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;

    let offset = from_proto::offset_from_position(
        position,
        &document.line_index,
        document.position_encoding,
    )?;

    let index = document.semantic_index();
    let targets =
        oak_ide::goto_definition(offset, &uri, &index, &state.root_scope(), &state.library);

    if targets.is_empty() {
        return Ok(None);
    }

    let links = targets
        .into_iter()
        .map(|target| nav_target_to_link(target, state))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(GotoDefinitionResponse::Link(links)))
}

fn nav_target_to_link(
    target: NavigationTarget,
    state: &WorldState,
) -> anyhow::Result<LocationLink> {
    let doc = state.get_document(&target.file)?;

    let target_range = to_proto::range(target.full_range, &doc.line_index, doc.position_encoding)?;
    let target_selection_range =
        to_proto::range(target.focus_range, &doc.line_index, doc.position_encoding)?;

    Ok(LocationLink {
        origin_selection_range: None,
        target_uri: target.file,
        target_range,
        target_selection_range,
    })
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

    fn make_state(uri: &lsp_types::Url, doc: &Document) -> WorldState {
        let mut state = WorldState::default();
        state.documents.insert(uri.clone(), doc.clone());
        state
    }

    #[test]
    fn test_goto_definition() {
        let code = "foo <- 42\nprint(foo)\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");
        let state = make_state(&uri, &doc);

        let params = make_params(uri, 1, 6);

        assert_matches!(
            goto_definition(&doc, params, &state).unwrap(),
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
        let state = make_state(&uri, &doc);

        let params = make_params(uri.clone(), 1, 0);

        assert_matches!(
            goto_definition(&doc, params, &state).unwrap(),
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
        let state = make_state(&uri, &doc);

        let params = make_params(uri, 0, 3);
        let result = goto_definition(&doc, params, &state).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_goto_definition_unresolved_returns_none() {
        let code = "foo\n";
        let doc = Document::new(code, None);
        let uri = test_path("test.R");
        let state = make_state(&uri, &doc);

        let params = make_params(uri, 0, 0);
        let result = goto_definition(&doc, params, &state).unwrap();
        assert_eq!(result, None);
    }
}
