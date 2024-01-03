//
// editor.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_shim::*;
use stdext::unwrap;
use tower_lsp::lsp_types::ShowDocumentParams;
use tower_lsp::lsp_types::Url;

use crate::lsp::globals::R_CALLBACK_GLOBALS;
use crate::lsp::globals::R_CALLBACK_GLOBALS2;

#[harp::register]
unsafe extern "C" fn ps_editor(file: SEXP, _title: SEXP) -> anyhow::Result<SEXP> {
    let globals = R_CALLBACK_GLOBALS.as_ref().unwrap();
    let client = &globals.lsp_client;
    let files = CharacterVector::new_unchecked(file);

    let globals2 = R_CALLBACK_GLOBALS2.as_ref().unwrap();
    let runtime = &globals2.lsp_runtime;

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
            client
                .show_document(ShowDocumentParams {
                    uri,
                    external: Some(false),
                    take_focus: Some(true),
                    selection: None,
                })
                .await
                .unwrap();
        });
    }

    Ok(R_NilValue)
}
