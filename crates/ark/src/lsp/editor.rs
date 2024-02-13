//
// editor.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use harp::vector::CharacterVector;
use harp::vector::Vector;
use libr::R_NilValue;
use libr::SEXP;
use stdext::unwrap;
use tower_lsp::lsp_types::ShowDocumentParams;
use tower_lsp::lsp_types::Url;

use crate::interface::RMain;

#[harp::register]
unsafe extern "C" fn ps_editor(file: SEXP, _title: SEXP) -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let runtime = main.get_lsp_runtime();

    let backend = unwrap!(main.get_lsp_backend(), None => {
        log::error!("Failed to open file. LSP backend has not been initialized.");
        return Ok(R_NilValue);
    });

    let files = CharacterVector::new_unchecked(file);

    let mut uris = Vec::new();

    for file in files.iter() {
        let Some(file) = file else {
            // `NA_character_` slipped through
            continue;
        };

        let uri = Url::from_file_path(&file);

        let uri = unwrap!(uri, Err(_) => {
            // The R side of this handles most issues, but we don't want to panic
            // if some unknown file path slips through.
            // `from_file_path()` doesn't return `Display`able errors, so we
            // can't necessarily give a good reason.
            log::error!("Can't open file at '{}'.", file);
            continue;
        });

        uris.push(uri);
    }

    // Spawn a task to open the files in the editor. We use `spawn()` rather
    // than `block_on()` because we need `ps_editor()` to return immediately
    // to unblock the main R thread. Otherwise, at the `await` points in
    // `show_document()`, it is possible for the runtime to execute a different
    // async task that requires R (like diagnostics in `enqueue_diagnostics()`),
    // which would cause a deadlock if R was being blocked here.
    // Using `spawn()` means that `file.edit()` and the `editor` R option hook
    // don't actually wait for the file to be fully opened, but that is
    // generally ok and is also what RStudio does.
    // https://github.com/posit-dev/positron/issues/1885
    for uri in uris.into_iter() {
        runtime.spawn(async move {
            let result = backend
                .client
                .show_document(ShowDocumentParams {
                    uri: uri.clone(),
                    external: Some(false),
                    take_focus: Some(true),
                    selection: None,
                })
                .await;

            if let Err(err) = result {
                // In the unlikely event that the LSP `client` hasn't been
                // initialized yet, or has shut down, we probably don't want
                // to crash ark.
                log::error!("Failed to open '{uri}' due to {err:?}");
            }
        });
    }

    Ok(R_NilValue)
}
