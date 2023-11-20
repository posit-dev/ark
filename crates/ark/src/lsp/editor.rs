//
// editor.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_sys::*;
use stdext::unwrap;
use tokio::runtime::Runtime;
use tower_lsp::lsp_types::ShowDocumentParams;
use tower_lsp::lsp_types::Url;

use crate::lsp::globals::R_CALLBACK_GLOBALS;

#[harp::register]
unsafe extern "C" fn ps_editor(file: SEXP, _title: SEXP) -> SEXP {
    let rt = Runtime::new().unwrap();
    let globals = R_CALLBACK_GLOBALS.as_ref().unwrap();
    let files = CharacterVector::new_unchecked(file);

    for file in files.iter() {
        if let Some(file) = file {
            rt.block_on(async move {
                let uri = Url::from_file_path(&file);

                let uri = unwrap!(uri, Err(_) => {
                    // The R side of this handles most issues, but we don't want to panic
                    // if some unknown file path slips through.
                    // `from_file_path()` doesn't return `Display`able errors, so we
                    // can't necessarily give a good reason.
                    log::error!("Can't open file at '{}'.", file);
                    return;
                });

                globals
                    .lsp_client
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
    }

    Ok(R_NilValue)
}
