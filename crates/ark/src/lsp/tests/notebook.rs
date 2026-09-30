use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::DidOpenNotebookDocumentParams;
use tower_lsp_server::ls_types::GotoDefinitionParams;
use tower_lsp_server::ls_types::GotoDefinitionResponse;
use tower_lsp_server::ls_types::NotebookCell;
use tower_lsp_server::ls_types::NotebookCellKind;
use tower_lsp_server::ls_types::NotebookDocument;
use tower_lsp_server::ls_types::TextDocumentItem;
use tower_lsp_server::ls_types::Uri;

use crate::lsp::goto_definition::goto_definition;
use crate::lsp::state::WorldState;
use crate::lsp::state_handlers::did_open_notebook;

const NOTEBOOK: &str = "file:///proj/analysis.ipynb";

fn cell_uri(handle: usize) -> Uri {
    format!("vscode-notebook-cell:/proj/analysis.ipynb#W{handle}s")
        .parse()
        .unwrap()
}

fn code_cell(handle: usize) -> NotebookCell {
    NotebookCell {
        kind: NotebookCellKind::CODE,
        document: cell_uri(handle),
        metadata: None,
        execution_summary: None,
    }
}

fn cell_item(handle: usize, text: &str) -> TextDocumentItem {
    TextDocumentItem {
        uri: cell_uri(handle),
        language_id: "r".to_string(),
        version: 0,
        text: text.to_string(),
    }
}

/// Open a notebook whose cells are `cells` (all listed in the notebook) and
/// whose text documents are `texts` (the cells the client syncs).
fn open(state: &mut WorldState, cells: &[usize], texts: &[(usize, &str)]) {
    let params = DidOpenNotebookDocumentParams {
        notebook_document: NotebookDocument {
            uri: NOTEBOOK.parse().unwrap(),
            notebook_type: "jupyter-notebook".to_string(),
            version: 0,
            metadata: None,
            cells: cells.iter().map(|&handle| code_cell(handle)).collect(),
        },
        cell_text_documents: texts
            .iter()
            .map(|&(handle, text)| cell_item(handle, text))
            .collect(),
    };
    did_open_notebook(params, state).unwrap();
}

/// The URI Go to Definition lands on from `(line, character)` in `cell`, if any.
fn definition_uri(state: &WorldState, cell: usize, line: u32, character: u32) -> Option<Uri> {
    let params = GotoDefinitionParams {
        text_document_position_params: lsp_types::TextDocumentPositionParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: cell_uri(cell),
            },
            position: lsp_types::Position::new(line, character),
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    match goto_definition(params, state).unwrap() {
        None => None,
        Some(GotoDefinitionResponse::Link(links)) => Some(links[0].target_uri.clone()),
        Some(other) => panic!("unexpected response: {other:?}"),
    }
}

#[test]
fn test_notebook_did_open_resolves_across_cells() {
    let mut state = WorldState::default();
    open(&mut state, &[0, 1], &[
        (0, "helper <- function(a) a * 2\n"),
        (1, "helper(3)\n"),
    ]);

    assert_eq!(definition_uri(&state, 1, 0, 0), Some(cell_uri(0)));
}

#[test]
fn test_notebook_did_open_skips_cells_without_text_documents() {
    // A Python chunk (handle 1) sits between two R chunks. The client lists
    // only synced cells, but guard against a listed cell with no text.
    let mut state = WorldState::default();
    open(&mut state, &[0, 1, 2], &[(0, "x <- 1\n"), (2, "x\n")]);

    assert_eq!(definition_uri(&state, 2, 0, 0), Some(cell_uri(0)));
}

#[test]
fn test_notebook_did_open_registers_cells_as_open_files() {
    let mut state = WorldState::default();
    open(&mut state, &[0], &[(0, "x <- 1\n")]);

    assert_eq!(state.open_files.len(), 1);
    assert_eq!(state.notebooks.len(), 1);
}
