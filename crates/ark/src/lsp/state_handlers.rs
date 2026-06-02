//
// state_handlers.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashSet;
use std::path::PathBuf;

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::PositionEncoding;
use aether_path::FilePath;
use anyhow::anyhow;
use oak_scan::DbScan;
use oak_scan::FileEvent;
use oak_scan::FileEventKind;
use oak_semantic::package::Package;
use stdext::result::ResultExt;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::CompletionOptions;
use tower_lsp::lsp_types::CompletionOptionsCompletionItem;
use tower_lsp::lsp_types::DidChangeConfigurationParams;
use tower_lsp::lsp_types::DidChangeTextDocumentParams;
use tower_lsp::lsp_types::DidChangeWatchedFilesParams;
use tower_lsp::lsp_types::DidChangeWorkspaceFoldersParams;
use tower_lsp::lsp_types::DidCloseTextDocumentParams;
use tower_lsp::lsp_types::DidOpenTextDocumentParams;
use tower_lsp::lsp_types::DocumentOnTypeFormattingOptions;
use tower_lsp::lsp_types::ExecuteCommandOptions;
use tower_lsp::lsp_types::FileChangeType;
use tower_lsp::lsp_types::FoldingRangeProviderCapability;
use tower_lsp::lsp_types::FormattingOptions;
use tower_lsp::lsp_types::HoverProviderCapability;
use tower_lsp::lsp_types::ImplementationProviderCapability;
use tower_lsp::lsp_types::InitializeParams;
use tower_lsp::lsp_types::InitializeResult;
use tower_lsp::lsp_types::OneOf;
use tower_lsp::lsp_types::RenameOptions;
use tower_lsp::lsp_types::SelectionRangeProviderCapability;
use tower_lsp::lsp_types::ServerCapabilities;
use tower_lsp::lsp_types::ServerInfo;
use tower_lsp::lsp_types::SignatureHelpOptions;
use tower_lsp::lsp_types::TextDocumentSyncCapability;
use tower_lsp::lsp_types::TextDocumentSyncKind;
use tower_lsp::lsp_types::WorkDoneProgressOptions;
use tower_lsp::lsp_types::WorkspaceFoldersServerCapabilities;
use tower_lsp::lsp_types::WorkspaceServerCapabilities;
use tracing::Instrument;
use url::Url;

use crate::console::ConsoleNotification;
use crate::lsp;
use crate::lsp::backend::LspResult;
use crate::lsp::capabilities::Capabilities;
use crate::lsp::config::indent_style_from_lsp;
use crate::lsp::config::DOCUMENT_SETTINGS;
use crate::lsp::config::GLOBAL_SETTINGS;
use crate::lsp::inputs::source_root::SourceRoot;
use crate::lsp::main_loop::dispatch_scan_requests;
use crate::lsp::main_loop::DidCloseVirtualDocumentParams;
use crate::lsp::main_loop::DidOpenVirtualDocumentParams;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::state::workspace_uris;
use crate::lsp::state::WorldState;

// Handlers that mutate the world state

/// Information sent from the kernel to the LSP after each top-level evaluation.
#[derive(Debug)]
pub struct ConsoleInputs {
    /// List of console scopes, from innermost (global or debug) to outermost
    /// scope. Currently the scopes are vectors of symbol names. TODO: In the
    /// future, we should send structural information like search path, and let
    /// the LSP query us for the contents so that the LSP can cache the
    /// information.
    pub console_scopes: Vec<Vec<String>>,

    /// Packages currently installed in the library path. TODO: Should send
    /// library paths instead and inspect and cache package information in the LSP.
    pub installed_packages: Vec<String>,
}

// Handlers taking exclusive references to global state

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn initialize(
    params: InitializeParams,
    lsp_state: &mut LspState,
    state: &mut WorldState,
    events_tx: &TokioUnboundedSender<Event>,
) -> LspResult<InitializeResult> {
    let workspace_uris = effective_workspace_uris(&params);
    lsp_state.capabilities = Capabilities::new(params.capabilities);

    // Initialize the workspace folders
    let mut workspace_paths: Vec<PathBuf> = Vec::new();

    for uri in workspace_uris {
        state.workspace.folders.push(uri.clone());
        if let Ok(path) = uri.to_file_path() {
            workspace_paths.push(path.clone());
            // Try to load package from this workspace folder and set as
            // root if found. This means we're dealing with a package
            // source.
            if state.root.is_none() {
                match Package::load_from_folder(&path) {
                    Ok(Some(pkg)) => {
                        log::info!(
                            "Root: Loaded package `{pkg}` from {path} as project root",
                            pkg = pkg.description().name,
                            path = path.display()
                        );
                        state.root = Some(SourceRoot::Package(pkg));
                    },
                    Ok(None) => {
                        log::info!(
                            "Root: No package found at {path}, treating as folder of scripts",
                            path = path.display()
                        );
                    },
                    Err(err) => {
                        log::warn!(
                            "Root: Error loading package at {path}: {err}",
                            path = path.display()
                        );
                    },
                }
            }
        }
    }

    // Kick off the initial workspace scan
    let editor_owned: HashSet<FilePath> = state.open_files.keys().cloned().collect();
    let requests =
        lsp_state
            .oak_scheduler
            .set_workspace_paths(&mut state.db, &workspace_paths, &editor_owned);
    dispatch_scan_requests(events_tx, requests);

    let result = InitializeResult {
        server_info: Some(ServerInfo {
            name: "Ark R Kernel".to_string(),
            version: Some(crate::BUILD_VERSION.to_string()),
        }),
        capabilities: ServerCapabilities {
            // Currently hard-coded to UTF-16, but we might want to allow UTF-8 frontends
            // once/if Ark becomes an independent LSP
            position_encoding: Some(lsp_types::PositionEncodingKind::UTF16),
            text_document_sync: Some(TextDocumentSyncCapability::Kind(
                TextDocumentSyncKind::INCREMENTAL,
            )),
            selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
            hover_provider: Some(HoverProviderCapability::from(true)),
            completion_provider: Some(CompletionOptions {
                resolve_provider: Some(true),
                trigger_characters: Some(vec!["$".to_string(), "@".to_string(), ":".to_string()]),
                work_done_progress_options: Default::default(),
                all_commit_characters: None,
                completion_item: Some(CompletionOptionsCompletionItem {
                    label_details_support: Some(true),
                }),
            }),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_string(), ",".to_string(), "=".to_string()]),
                retrigger_characters: None,
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: None,
                },
            }),
            definition_provider: Some(OneOf::Left(true)),
            type_definition_provider: None,
            implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
            references_provider: Some(OneOf::Left(true)),
            rename_provider: Some(OneOf::Right(RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: None,
                },
            })),
            document_symbol_provider: Some(OneOf::Left(true)),
            folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
            execute_command_provider: Some(ExecuteCommandOptions {
                commands: vec![],
                work_done_progress_options: Default::default(),
            }),
            code_action_provider: lsp_state.capabilities.code_action_provider_capability(),
            workspace: Some(WorkspaceServerCapabilities {
                workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                    supported: Some(true),
                    change_notifications: Some(OneOf::Left(true)),
                }),
                // We don't register `file_operations`. Disk changes reach us
                // through `didChangeWatchedFiles` from every source (editor, git,
                // terminal), so it's the single channel that keeps the index
                // current. A rename arrives there as delete + create.
                file_operations: None,
            }),
            document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
                first_trigger_character: String::from("\n"),
                more_trigger_character: None,
            }),
            ..ServerCapabilities::default()
        },
    };

    Ok(result)
}

/// Resolve the effective workspace folders from `InitializeParams`.
///
/// We read only `workspaceFolders`, the modern field , without falling back to
/// the deprecated `rootUri`. An empty or absent list means single-file mode.
pub(super) fn effective_workspace_uris(params: &InitializeParams) -> Vec<Url> {
    params
        .workspace_folders
        .iter()
        .flatten()
        .map(|folder| folder.uri.clone())
        .collect()
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_open(
    params: DidOpenTextDocumentParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let contents = params.text_document.text;
    let uri = params.text_document.uri;
    let version = params.text_document.version;

    let file = state.db.upsert_editor(FilePath::from_url(&uri), contents);
    state.insert_ark_file(uri.clone(), file, Some(version));

    // NOTE: Do we need to call `update_config()` here?
    // update_config(vec![uri]).await;

    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change(
    params: DidChangeTextDocumentParams,
    lsp_state: &mut LspState,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let uri = &params.text_document.uri;
    let key = FilePath::from_url(uri);
    let new_version = params.text_document.version;
    let encoding = state.config.position_encoding;

    let Some(file) = state.open_files.get_mut(&key) else {
        return Err(anyhow!("Can't find document for URI {uri}"));
    };

    // Reject out-of-order change notifications. The spec allows version numbers
    // to skip values but requires them to increase monotonically. A lower
    // version means we've lost sync and can't keep our state consistent.
    // Currently panicking, but in principle we should shut the LSP down in an
    // orderly fashion.
    if let Some(old_version) = file.version {
        if new_version < old_version {
            panic!(
                "out-of-sync change notification: currently at {old_version}, got {new_version}"
            );
        }
    }

    // Fold the edits into the new buffer text and push it into `oak`
    let new_contents =
        apply_content_changes(file.contents(&state.db), &params.content_changes, encoding);
    state.db.upsert_editor(key.clone(), new_contents);

    file.version = Some(new_version);

    // Notify console about document change to invalidate breakpoints.
    lsp_state
        .console_notification_tx
        .send(ConsoleNotification::DidChangeDocument(key))
        .log_err();

    Ok(())
}

// --- source
// authors = ["rust-analyzer team"]
// license = "MIT OR Apache-2.0"
// origin = "https://github.com/rust-lang/rust-analyzer/blob/master/crates/rust-analyzer/src/lsp/utils.rs"
// ---
/// Apply a batch of LSP content changes to `contents`, returning the new text.
fn apply_content_changes(
    contents: &str,
    content_changes: &[lsp_types::TextDocumentContentChangeEvent],
    encoding: PositionEncoding,
) -> String {
    let mut contents = contents.to_string();
    let mut changes = content_changes.to_vec();

    // If at least one of the changes is a full document change, use the last of them
    // as the starting point and ignore all previous changes. We then know that all
    // changes after this (if any!) are incremental changes.
    //
    // If we do have a full document change, that implies the `last_start_line`
    // corresponding to that change is line 0, which will correctly force a rebuild
    // of the line index before applying any incremental changes.
    let (changes, mut last_start_line) =
        match changes.iter().rposition(|change| change.range.is_none()) {
            Some(idx) => {
                let incremental = changes.split_off(idx + 1);
                // Unwrap: `rposition()` confirmed this index contains a full document change
                let change = changes.pop().unwrap();
                contents = change.text;
                (incremental, 0)
            },
            None => (changes, u32::MAX),
        };

    let mut line_index = biome_line_index::LineIndex::new(&contents);

    // Handle all incremental changes after the last full document change. We don't
    // typically get >1 incremental change as the user types, but we do get them in a
    // batch after a find-and-replace, or after a format-on-save request.
    //
    // Some editors like VS Code send the edits in reverse order (from the bottom of
    // file -> top of file). We can take advantage of this, because applying an edit
    // on, say, line 10, doesn't invalidate the `line_index` if we then need to apply
    // an additional edit on line 5. That said, we may still have edits that cross
    // lines, so rebuilding the `line_index` is not always unavoidable.
    for change in changes {
        let range = change
            .range
            .expect("`None` case already handled by finding the last full document change.");

        // If the end of this change is at or past the start of the last change, then
        // the `line_index` needed to apply this change is now invalid, so we have to
        // rebuild it.
        if range.end.line >= last_start_line {
            line_index = biome_line_index::LineIndex::new(&contents);
        }
        last_start_line = range.start.line;

        // This is a panic if we can't convert. It means we can't keep the document up
        // to date and something is very wrong.
        let range: std::ops::Range<usize> = from_proto::text_range(range, &line_index, encoding)
            .expect("Can convert `range` from `Position` to `TextRange`.")
            .into();

        contents.replace_range(range, &change.text);
    }

    contents
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_close(
    params: DidCloseTextDocumentParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let uri = params.text_document.uri;

    // Publish empty set of diagnostics to clear them
    lsp::publish_diagnostics(uri.clone(), Vec::new(), None);

    state
        .open_files
        .remove(&FilePath::from_url(&uri))
        .ok_or(anyhow!("Failed to remove document for URI: {uri}"))?;

    let path = FilePath::from_url(&uri);
    state.db.close_editor(&path);

    lsp::log_info!("did_close(): closed document with URI: '{uri}'.");

    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change_watched_files(
    params: DidChangeWatchedFilesParams,
    state: &mut WorldState,
    lsp_state: &mut LspState,
    events_tx: &TokioUnboundedSender<Event>,
) -> anyhow::Result<()> {
    // Editor owns the contents of files it has open: ignore disk-side events
    // for those URLs. Their content comes from `did_open` / `did_change`.
    let editor_owned: HashSet<FilePath> = state.open_files.keys().cloned().collect();

    let events: Vec<FileEvent> = params
        .changes
        .iter()
        .filter_map(|change| {
            Some(FileEvent {
                path: FilePath::from_url(&change.uri),
                kind: file_event_kind(change.typ)?,
            })
        })
        .collect();

    let requests =
        lsp_state
            .oak_scheduler
            .apply_watcher_events(&mut state.db, events, &editor_owned);
    dispatch_scan_requests(events_tx, requests);

    Ok(())
}

fn file_event_kind(kind: FileChangeType) -> Option<FileEventKind> {
    match kind {
        FileChangeType::CREATED => Some(FileEventKind::Created),
        FileChangeType::CHANGED => Some(FileEventKind::Changed),
        FileChangeType::DELETED => Some(FileEventKind::Deleted),
        _ => None,
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change_workspace_folders(
    params: DidChangeWorkspaceFoldersParams,
    state: &mut WorldState,
    lsp_state: &mut LspState,
    events_tx: &TokioUnboundedSender<Event>,
) -> anyhow::Result<()> {
    let removed: HashSet<Url> = params.event.removed.iter().map(|f| f.uri.clone()).collect();
    state.workspace.folders.retain(|uri| !removed.contains(uri));

    for folder in params.event.added {
        if !state.workspace.folders.contains(&folder.uri) {
            state.workspace.folders.push(folder.uri);
        }
    }

    let workspace_paths: Vec<PathBuf> = state
        .workspace
        .folders
        .iter()
        .filter_map(|uri| uri.to_file_path().ok())
        .collect();

    // Editor-owned URLs survive eviction in `OrphanRoot` so the user's
    // open buffers keep getting analysed even when their workspace
    // folder goes away.
    let editor_owned: HashSet<FilePath> = state.open_files.keys().cloned().collect();

    let requests =
        lsp_state
            .oak_scheduler
            .set_workspace_paths(&mut state.db, &workspace_paths, &editor_owned);
    dispatch_scan_requests(events_tx, requests);
    Ok(())
}

pub(crate) async fn did_change_configuration(
    _params: DidChangeConfigurationParams,
    client: &tower_lsp::Client,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    // The notification params sometimes contain data but it seems in practice
    // we should just ignore it. Instead we need to pull the settings again for
    // all URI of interest.

    // Note that the client sends notifications for settings for which we have
    // declared interest in. This registration is done in `handle_initialized()`.

    update_config(workspace_uris(state), client, state)
        .instrument(tracing::info_span!("did_change_configuration"))
        .await
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change_formatting_options(
    uri: &Url,
    opts: &FormattingOptions,
    state: &mut WorldState,
) {
    let Ok(doc) = state.ark_file_mut(uri) else {
        return;
    };

    // The information provided in formatting requests is more up-to-date
    // than the user settings because it also includes changes made to the
    // configuration of particular editors. However the former is less rich
    // than the latter: it does not allow the tab size to differ from the
    // indent size, as in the R core sources. So we just ignore the less
    // rich updates in this case.
    if doc.config.indent.indent_size != doc.config.indent.tab_width {
        return;
    }

    doc.config.indent.indent_size = opts.tab_size as usize;
    doc.config.indent.tab_width = opts.tab_size as usize;
    doc.config.indent.indent_style = indent_style_from_lsp(opts.insert_spaces);

    // TODO:
    // `trim_trailing_whitespace`
    // `trim_final_newlines`
    // `insert_final_newline`
}

async fn update_config(
    uris: Vec<Url>,
    client: &tower_lsp::Client,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    // Keep track of existing config to detect whether it was changed
    let diagnostics_config = state.config.diagnostics.clone();

    // Build the configuration request for global and document settings
    let mut items: Vec<_> = vec![];

    // This should be first because we first handle the global settings below,
    // splitting them off the response array
    let mut global_items: Vec<_> = GLOBAL_SETTINGS
        .iter()
        .map(|mapping| lsp_types::ConfigurationItem {
            scope_uri: None,
            section: Some(mapping.key.to_string()),
        })
        .collect();

    // For document items we create a n_uris * n_document_settings array that we'll
    // handle by batch in a double loop over URIs and document settings
    let mut document_items: Vec<_> = uris
        .iter()
        .flat_map(|uri| {
            DOCUMENT_SETTINGS
                .iter()
                .map(|mapping| lsp_types::ConfigurationItem {
                    scope_uri: Some(uri.clone()),
                    section: Some(mapping.key.to_string()),
                })
        })
        .collect();

    // Concatenate everything into a flat array that we'll send in one request
    items.append(&mut global_items);
    items.append(&mut document_items);

    // The response better match the number of items we send in
    let n_items = items.len();

    let mut configs = client.configuration(items).await?;

    if configs.len() != n_items {
        return Err(anyhow!(
            "Unexpected number of retrieved configurations: {}/{}",
            configs.len(),
            n_items
        ));
    }

    let document_configs = configs.split_off(GLOBAL_SETTINGS.len());
    let global_configs = configs;

    for (mapping, value) in GLOBAL_SETTINGS.iter().zip(global_configs) {
        (mapping.set)(&mut state.config, value);
    }

    let mut remaining = document_configs;

    for uri in uris.into_iter() {
        // Need to juggle a bit because `split_off()` returns the tail of the
        // split and updates the vector with the head
        let tail = remaining.split_off(DOCUMENT_SETTINGS.len());
        let head = std::mem::replace(&mut remaining, tail);

        for (mapping, value) in DOCUMENT_SETTINGS.iter().zip(head) {
            if let Ok(doc) = state.ark_file_mut(&uri) {
                (mapping.set)(&mut doc.config, value);
            }
        }
    }

    // Refresh diagnostics if the configuration changed
    if state.config.diagnostics != diagnostics_config {
        tracing::info!("Refreshing diagnostics after configuration changed");
        lsp::main_loop::diagnostics_refresh_all(state);
    }

    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change_console_inputs(
    inputs: ConsoleInputs,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    state.console_scopes = inputs.console_scopes;
    state.installed_packages = inputs.installed_packages;

    // We currently rely on global console scopes for diagnostics, in particular
    // during package development in conjunction with `devtools::load_all()`.
    // Ideally diagnostics would not rely on these though, and we wouldn't need
    // to refresh from here.
    lsp::diagnostics_refresh_all(state);

    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_open_virtual_document(
    params: DidOpenVirtualDocumentParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    // Insert new document, replacing any old one
    state.virtual_documents.insert(params.uri, params.contents);
    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_close_virtual_document(
    params: DidCloseVirtualDocumentParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    state.virtual_documents.remove(&params.uri);
    Ok(())
}

#[cfg(test)]
mod tests {
    use biome_line_index::WideEncoding;

    use super::*;

    const ENCODING: PositionEncoding = PositionEncoding::Wide(WideEncoding::Utf16);

    fn insert(text: &str, line: u32, character: u32) -> lsp_types::TextDocumentContentChangeEvent {
        let position = lsp_types::Position::new(line, character);
        lsp_types::TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range::new(position, position)),
            range_length: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn test_apply_content_changes_incremental_inserts() {
        // Type "lib" one character at a time, the way an editor streams it.
        let after_l = apply_content_changes("", &[insert("l", 0, 0)], ENCODING);
        assert_eq!(after_l, "l");

        let after_i = apply_content_changes(&after_l, &[insert("i", 0, 1)], ENCODING);
        assert_eq!(after_i, "li");

        let after_b = apply_content_changes(&after_i, &[insert("b", 0, 2)], ENCODING);
        assert_eq!(after_b, "lib");
    }

    #[test]
    fn test_apply_content_changes_full_replacement_wins() {
        // A range-less change replaces the whole buffer; earlier changes in the
        // batch are discarded, later incremental ones apply on top of it.
        let changes = vec![
            insert("ignored", 0, 0),
            lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "abc\n".to_string(),
            },
            insert("X", 0, 3),
        ];
        assert_eq!(apply_content_changes("old", &changes, ENCODING), "abcX\n");
    }
}
