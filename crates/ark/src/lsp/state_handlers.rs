//
// state_handlers.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use tower_lsp::lsp_types;
use tower_lsp::lsp_types::CompletionOptions;
use tower_lsp::lsp_types::CompletionOptionsCompletionItem;
use tower_lsp::lsp_types::DidChangeConfigurationParams;
use tower_lsp::lsp_types::DidChangeTextDocumentParams;
use tower_lsp::lsp_types::DidCloseTextDocumentParams;
use tower_lsp::lsp_types::DidOpenTextDocumentParams;
use tower_lsp::lsp_types::DocumentOnTypeFormattingOptions;
use tower_lsp::lsp_types::ExecuteCommandOptions;
use tower_lsp::lsp_types::FoldingRangeProviderCapability;
use tower_lsp::lsp_types::FormattingOptions;
use tower_lsp::lsp_types::HoverProviderCapability;
use tower_lsp::lsp_types::ImplementationProviderCapability;
use tower_lsp::lsp_types::InitializeParams;
use tower_lsp::lsp_types::InitializeResult;
use tower_lsp::lsp_types::OneOf;
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
use tree_sitter::Parser;
use url::Url;

use crate::lsp;
use crate::lsp::capabilities::Capabilities;
use crate::lsp::config::indent_style_from_lsp;
use crate::lsp::config::DOCUMENT_SETTINGS;
use crate::lsp::config::GLOBAL_SETTINGS;
use crate::lsp::documents::Document;
use crate::lsp::encoding::get_position_encoding_kind;
use crate::lsp::inputs::package::Package;
use crate::lsp::inputs::source_root::SourceRoot;
use crate::lsp::main_loop::DidCloseVirtualDocumentParams;
use crate::lsp::main_loop::DidOpenVirtualDocumentParams;
use crate::lsp::main_loop::LspState;
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
) -> anyhow::Result<InitializeResult> {
    lsp_state.capabilities = Capabilities::new(params.capabilities);

    // Initialize the workspace folders
    let mut folders: Vec<String> = Vec::new();
    if let Some(workspace_folders) = params.workspace_folders {
        for folder in workspace_folders.iter() {
            state.workspace.folders.push(folder.uri.clone());
            if let Ok(path) = folder.uri.to_file_path() {
                // Try to load package from this workspace folder and set as
                // root if found. This means we're dealing with a package
                // source.
                if state.root.is_none() {
                    match Package::load(&path) {
                        Ok(Some(pkg)) => {
                            log::info!(
                                "Root: Loaded package `{pkg}` from {path} as project root",
                                pkg = pkg.description.name,
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
                if let Some(path_str) = path.to_str() {
                    folders.push(path_str.to_string());
                }
            }
        }
    }

    // Start first round of indexing
    lsp::main_loop::index_start(folders, state.clone());

    Ok(InitializeResult {
        server_info: Some(ServerInfo {
            name: "Ark R Kernel".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
        capabilities: ServerCapabilities {
            position_encoding: Some(get_position_encoding_kind()),
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
                ..Default::default()
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
                file_operations: None,
            }),
            document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
                first_trigger_character: String::from("\n"),
                more_trigger_character: None,
            }),
            ..ServerCapabilities::default()
        },
    })
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_open(
    params: DidOpenTextDocumentParams,
    lsp_state: &mut LspState,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let contents = params.text_document.text.as_str();
    let uri = params.text_document.uri;
    let version = params.text_document.version;

    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .unwrap();

    let document = Document::new_with_parser(contents, &mut parser, Some(version));

    lsp_state.parsers.insert(uri.clone(), parser);
    state.documents.insert(uri.clone(), document.clone());

    // NOTE: Do we need to call `update_config()` here?
    // update_config(vec![uri]).await;

    lsp::main_loop::index_update(uri.clone(), document.clone(), state.clone());

    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_change(
    params: DidChangeTextDocumentParams,
    lsp_state: &mut LspState,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let uri = &params.text_document.uri;
    let document = state.get_document_mut(uri)?;

    let mut parser = lsp_state
        .parsers
        .get_mut(uri)
        .ok_or(anyhow!("No parser for {uri}"))?;

    document.on_did_change(&mut parser, &params);

    lsp::main_loop::index_update(uri.clone(), document.clone(), state.clone());

    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn did_close(
    params: DidCloseTextDocumentParams,
    lsp_state: &mut LspState,
    state: &mut WorldState,
) -> anyhow::Result<()> {
    let uri = params.text_document.uri;

    // Publish empty set of diagnostics to clear them
    lsp::publish_diagnostics(uri.clone(), Vec::new(), None);

    state
        .documents
        .remove(&uri)
        .ok_or(anyhow!("Failed to remove document for URI: {uri}"))?;

    lsp_state
        .parsers
        .remove(&uri)
        .ok_or(anyhow!("Failed to remove parser for URI: {uri}"))?;

    lsp::log_info!("did_close(): closed document with URI: '{uri}'.");

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
    let Ok(doc) = state.get_document_mut(uri) else {
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

    for (mapping, value) in GLOBAL_SETTINGS.into_iter().zip(global_configs) {
        (mapping.set)(&mut state.config, value);
    }

    let mut remaining = document_configs;

    for uri in uris.into_iter() {
        // Need to juggle a bit because `split_off()` returns the tail of the
        // split and updates the vector with the head
        let tail = remaining.split_off(DOCUMENT_SETTINGS.len());
        let head = std::mem::replace(&mut remaining, tail);

        for (mapping, value) in DOCUMENT_SETTINGS.iter().zip(head) {
            if let Ok(doc) = state.get_document_mut(&uri) {
                (mapping.set)(&mut doc.config, value);
            }
        }
    }

    // Refresh diagnostics if the configuration changed
    if state.config.diagnostics != diagnostics_config {
        tracing::info!("Refreshing diagnostics after configuration changed");
        lsp::main_loop::diagnostics_refresh_all(state.clone());
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
    lsp::diagnostics_refresh_all(state.clone());

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
