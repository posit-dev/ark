//
// state_handlers.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashSet;
use std::path::PathBuf;

use aether_path::AbsPathBuf;
use aether_path::FilePath;
use anyhow::anyhow;
use oak_scan::DbScan;
use oak_scan::FileEvent;
use oak_scan::FileEventKind;
use stdext::result::ResultExt;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::CompletionOptions;
use tower_lsp_server::ls_types::CompletionOptionsCompletionItem;
use tower_lsp_server::ls_types::DidChangeConfigurationParams;
use tower_lsp_server::ls_types::DidChangeTextDocumentParams;
use tower_lsp_server::ls_types::DidChangeWatchedFilesParams;
use tower_lsp_server::ls_types::DidChangeWatchedFilesRegistrationOptions;
use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::DidCloseTextDocumentParams;
use tower_lsp_server::ls_types::DidOpenTextDocumentParams;
use tower_lsp_server::ls_types::DocumentOnTypeFormattingOptions;
use tower_lsp_server::ls_types::ExecuteCommandOptions;
use tower_lsp_server::ls_types::FileChangeType;
use tower_lsp_server::ls_types::FileSystemWatcher;
use tower_lsp_server::ls_types::FoldingRangeProviderCapability;
use tower_lsp_server::ls_types::FormattingOptions;
use tower_lsp_server::ls_types::GlobPattern;
use tower_lsp_server::ls_types::HoverProviderCapability;
use tower_lsp_server::ls_types::ImplementationProviderCapability;
use tower_lsp_server::ls_types::InitializeParams;
use tower_lsp_server::ls_types::InitializeResult;
use tower_lsp_server::ls_types::OneOf;
use tower_lsp_server::ls_types::Registration;
use tower_lsp_server::ls_types::RenameOptions;
use tower_lsp_server::ls_types::SelectionRangeProviderCapability;
use tower_lsp_server::ls_types::ServerCapabilities;
use tower_lsp_server::ls_types::ServerInfo;
use tower_lsp_server::ls_types::SignatureHelpOptions;
use tower_lsp_server::ls_types::TextDocumentSyncCapability;
use tower_lsp_server::ls_types::TextDocumentSyncKind;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::WorkDoneProgressOptions;
use tower_lsp_server::ls_types::WorkspaceFoldersServerCapabilities;
use tower_lsp_server::ls_types::WorkspaceServerCapabilities;
use tracing::Instrument;

use crate::console::ConsoleNotification;
use crate::lsp;
use crate::lsp::backend::LspResult;
use crate::lsp::capabilities::Capabilities;
use crate::lsp::config::apply_env_overrides;
use crate::lsp::config::indent_style_from_lsp;
use crate::lsp::config::DOCUMENT_SETTINGS;
use crate::lsp::config::GLOBAL_SETTINGS;
use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;
use crate::lsp::content_changes::apply_content_changes;
use crate::lsp::main_loop::dispatch_scan_requests;
use crate::lsp::main_loop::DiagnosticsPublication;
use crate::lsp::main_loop::DidCloseVirtualDocumentParams;
use crate::lsp::main_loop::DidOpenVirtualDocumentParams;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::sources::source_fetching_disabled_by_ci;
use crate::lsp::state::open_file_wire_uris;
use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

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
    let workspace_paths = effective_workspace_paths(&params);
    lsp_state.capabilities = Capabilities::new(params.capabilities);

    state.workspace.folders = workspace_paths;

    dispatch_workspace_scan(state, lsp_state, events_tx);

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
        offset_encoding: None,
    };

    Ok(result)
}

/// Resolve the effective workspace folders from `InitializeParams`.
///
/// We read only `workspaceFolders`, the modern field , without falling back to
/// the deprecated `rootUri`. An empty or absent list means single-file mode.
/// A folder that isn't a `file:` URI, or whose path we can't make sense of, is
/// silently dropped: it never backs a directory we could scan.
pub(super) fn effective_workspace_paths(params: &InitializeParams) -> Vec<AbsPathBuf> {
    params
        .workspace_folders
        .iter()
        .flatten()
        .filter_map(|folder| folder.uri.to_url().log_err())
        .filter_map(|url| AbsPathBuf::from_url(&url))
        .collect()
}

pub(crate) async fn handle_initialized(
    client: &tower_lsp_server::Client,
    lsp_state: &mut LspState,
    state: &mut WorldState,
    events_tx: &TokioUnboundedSender<Event>,
) {
    let span = tracing::info_span!("handle_initialized").entered();

    // Register capabilities to the client
    let mut regs: Vec<Registration> = vec![];

    // Watch R files and DESCRIPTION. We get notified on any disk change;
    // the handler skips editor-owned URLs since those are tracked via
    // `textDocument/did*` instead.
    let watchers = vec![
        FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*.{R,r}".to_string()),
            kind: None,
        },
        FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/DESCRIPTION".to_string()),
            kind: None,
        },
    ];
    regs.push(Registration {
        id: uuid::Uuid::new_v4().to_string(),
        method: String::from("workspace/didChangeWatchedFiles"),
        register_options: Some(
            serde_json::to_value(DidChangeWatchedFilesRegistrationOptions { watchers }).unwrap(),
        ),
    });

    if lsp_state
        .capabilities
        .dynamic_registration_for_did_change_configuration()
    {
        // The `didChangeConfiguration` request instructs the client to send
        // a notification when the tracked settings have changed.
        //
        // Note that some settings, such as editor indentation properties, may be
        // changed by extensions or by the user without changing the actual
        // underlying setting. Unfortunately we don't receive updates in that case.

        for setting in GLOBAL_SETTINGS {
            regs.push(Registration {
                id: uuid::Uuid::new_v4().to_string(),
                method: String::from("workspace/didChangeConfiguration"),
                register_options: Some(serde_json::json!({ "section": setting.key })),
            });
        }
        for setting in DOCUMENT_SETTINGS {
            regs.push(Registration {
                id: uuid::Uuid::new_v4().to_string(),
                method: String::from("workspace/didChangeConfiguration"),
                register_options: Some(serde_json::json!({ "section": setting.key })),
            });
        }
    }

    client
        .register_capability(regs)
        .instrument(span.exit())
        .await
        .log_err();

    update_config(
        open_file_wire_uris(state),
        client,
        &lsp_state.capabilities,
        state,
    )
    .await
    .log_err();

    // Release the startup gate after attempting to load client configuration.
    // Packages seen so far will be queued now if the scheduler is activated.
    lsp_state.source_scheduler.config_arrived();

    // Say once why nothing will be fetched, if that's the case.
    if !state.config.oak.source_fetching_enabled {
        lsp::log_info!("Source fetching is disabled by `oak.sourceFetching.enabled`");
    } else if source_fetching_disabled_by_ci() {
        lsp::log_info!(
            "Source fetching is disabled on CI. Set {OAK_SOURCE_FETCHING_ENABLED_ENV_VAR}=1 to enable it."
        );
    } else if !lsp_state.source_scheduler.has_handler() {
        // The specific reason (no R executable, or a handler that failed to
        // build) was logged to the kernel log.
        lsp::log_info!("Source fetching is unavailable, no source handler was built");
    }

    lsp_state.source_scheduler.schedule(
        &state.db,
        &state.config.oak,
        &lsp_state.source_pool,
        events_tx,
    );
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_open(
    params: DidOpenTextDocumentParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let contents = params.text_document.text;
    let wire_uri = params.text_document.uri;
    let path = wire_uri.to_document_path()?;
    let version = params.text_document.version;

    let file = state.db.upsert_editor(path.clone(), contents);
    state.insert_open_file(wire_uri, path, file, Some(version));

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
    let path = params.text_document.uri.to_document_path()?;
    let new_version = params.text_document.version;
    let encoding = state.config.position_encoding;

    let file = state.open_file(&path)?;

    // Reject out-of-order change notifications. The spec allows version numbers
    // to skip values but requires them to increase monotonically. A lower
    // version means we've lost sync and can't keep our state consistent.
    // Currently panicking, but in principle we should shut the LSP down in an
    // orderly fashion.
    if let Some(old_version) = file.version() {
        if new_version < old_version {
            panic!(
                "out-of-sync change notification: currently at {old_version}, got {new_version}"
            );
        }
    }

    // Fold the edits into the new buffer text and push it into `oak`
    let new_contents = apply_content_changes(
        file.source_text(&state.db).as_str(),
        &params.content_changes,
        encoding,
    );
    state.db.upsert_editor(path.clone(), new_contents);

    state.open_file_mut(&path)?.set_version(Some(new_version));

    // Notify console about document change to invalidate breakpoints.
    lsp_state
        .console_notification_tx
        .send(ConsoleNotification::DidChangeDocument(path))
        .log_err();

    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_close(
    params: DidCloseTextDocumentParams,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let wire_uri = params.text_document.uri;
    let path = wire_uri.to_document_path()?;

    // Publish empty set of diagnostics to clear them
    lsp::publish_diagnostics(DiagnosticsPublication {
        path: path.clone(),
        uri: wire_uri.clone(),
        diagnostics: Vec::new(),
        version: None,
    });

    state.open_files.remove(&path).ok_or(anyhow!(
        "Failed to remove document for URI: {}",
        wire_uri.as_str()
    ))?;

    state.db.close_editor(&path);

    lsp::log_info!(
        "did_close(): closed document with URI: '{}'.",
        wire_uri.as_str()
    );

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
                path: change.uri.to_document_path().log_err()?,
                kind: file_event_kind(change.typ)?,
            })
        })
        .collect();

    let requests =
        lsp_state
            .oak_scheduler
            .apply_watcher_events(&mut state.db, events, &editor_owned);
    dispatch_scan_requests(&lsp_state.scan_pool, events_tx, requests);

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
    let removed: HashSet<AbsPathBuf> = params
        .event
        .removed
        .iter()
        .filter_map(|f| f.uri.to_url().log_err())
        .filter_map(|url| AbsPathBuf::from_url(&url))
        .collect();
    state
        .workspace
        .folders
        .retain(|path| !removed.contains(path));

    for folder in params.event.added {
        let Some(url) = folder.uri.to_url().log_err() else {
            continue;
        };
        let Some(path) = AbsPathBuf::from_url(&url) else {
            continue;
        };
        if !state.workspace.folders.contains(&path) {
            state.workspace.folders.push(path);
        }
    }

    dispatch_workspace_scan(state, lsp_state, events_tx);
    Ok(())
}

/// Update scan roots from `state.workspace.folders`. Dispatch scans for files
/// entering or leaving workspace scope.
fn dispatch_workspace_scan(
    state: &mut WorldState,
    lsp_state: &mut LspState,
    events_tx: &TokioUnboundedSender<Event>,
) {
    // Editor-owned URLs survive eviction in `OrphanRoot` so the user's
    // open buffers keep getting analysed even when their workspace
    // folder goes away.
    let editor_owned: HashSet<FilePath> = state.open_files.keys().cloned().collect();

    let requests = lsp_state.oak_scheduler.set_workspace_paths(
        &mut state.db,
        &to_std_paths(&state.workspace.folders),
        &editor_owned,
    );
    dispatch_scan_requests(&lsp_state.scan_pool, events_tx, requests);
}

/// Convert workspace folders to the `std::path::PathBuf`s the scan scheduler
/// takes.
fn to_std_paths(paths: &[AbsPathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| path.as_path().as_std_path().to_path_buf())
        .collect()
}

pub(crate) async fn did_change_configuration(
    _params: DidChangeConfigurationParams,
    client: &tower_lsp_server::Client,
    capabilities: &Capabilities,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    // The notification params sometimes contain data but it seems in practice
    // we should just ignore it. Instead we need to pull the settings again for
    // all URI of interest.

    // Note that the client sends notifications for settings for which we have
    // declared interest in. This registration is done in `handle_initialized()`.

    update_config(open_file_wire_uris(state), client, capabilities, state)
        .instrument(tracing::info_span!("did_change_configuration"))
        .await
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change_formatting_options(
    path: &FilePath,
    opts: &FormattingOptions,
    state: &mut WorldState,
) {
    let Ok(doc) = state.open_file_mut(path) else {
        return;
    };

    // The information provided in formatting requests is more up-to-date
    // than the user settings because it also includes changes made to the
    // configuration of particular editors. However the former is less rich
    // than the latter: it does not allow the tab size to differ from the
    // indent size, as in the R core sources. So we just ignore the less
    // rich updates in this case.
    if doc.config().indent.indent_size != doc.config().indent.tab_width {
        return;
    }

    let indent = &mut doc.config_mut().indent;
    indent.indent_size = opts.tab_size as usize;
    indent.tab_width = opts.tab_size as usize;
    indent.indent_style = indent_style_from_lsp(opts.insert_spaces);

    // TODO:
    // `trim_trailing_whitespace`
    // `trim_final_newlines`
    // `insert_final_newline`
}

async fn update_config(
    open_files: Vec<(FilePath, Uri)>,
    client: &tower_lsp_server::Client,
    capabilities: &Capabilities,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    // Keep track of existing config to detect whether it was changed
    let diagnostics_config = state.config.diagnostics.clone();
    let oak_config = state.config.oak.clone();

    let pulled = if capabilities.workspace_configuration() {
        pull_config(open_files, client, state).await
    } else {
        lsp::log_info!("Client can't answer `workspace/configuration`, using default settings");
        Ok(())
    };

    // Overrides apply even when the pull failed, so that an unresponsive client
    // doesn't strand us on the defaults.
    apply_env_overrides(&mut state.config);

    if state.config.oak.source_fetching_enabled != oak_config.source_fetching_enabled {
        let state_name = if state.config.oak.source_fetching_enabled {
            "enabled"
        } else {
            "disabled"
        };
        lsp::log_info!("Source fetching {state_name} by `oak.sourceFetching.enabled`");
    }

    // `config` is not an Oak input, so we manually bump the revision to refresh
    // diagnostics and rerun source scheduling. This queues already discovered
    // packages when source fetching is re-enabled.
    if state.config.diagnostics != diagnostics_config || state.config.oak != oak_config {
        tracing::info!("Bumping salsa revision after configuration changed");
        state.bump_revision();
    }

    pulled
}

/// Pull the global and document settings from the client with a
/// `workspace/configuration` request, then store them in `state`.
async fn pull_config(
    open_files: Vec<(FilePath, Uri)>,
    client: &tower_lsp_server::Client,
    state: &mut WorldState,
) -> anyhow::Result<()> {
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
    let mut document_items: Vec<_> = open_files
        .iter()
        .flat_map(|(_path, uri)| {
            DOCUMENT_SETTINGS
                .iter()
                .map(move |mapping| lsp_types::ConfigurationItem {
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

    for (path, _uri) in open_files {
        // Need to juggle a bit because `split_off()` returns the tail of the
        // split and updates the vector with the head
        let tail = remaining.split_off(DOCUMENT_SETTINGS.len());
        let head = std::mem::replace(&mut remaining, tail);

        for (mapping, value) in DOCUMENT_SETTINGS.iter().zip(head) {
            if let Ok(doc) = state.open_file_mut(&path) {
                (mapping.set)(doc.config_mut(), value);
            }
        }
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
    // to refresh from here. The scopes live outside oak so we bump the revision
    // manually, causing diagnostics to get refreshed on the next tick.
    state.bump_revision();

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
