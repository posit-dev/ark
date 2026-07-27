//
// url.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;
use std::result::Result::Ok;

use anyhow::*;
use stdext::unwrap;
use tower_lsp_server::ls_types::Uri;
use url::Url;

pub trait UrlExt {
    fn file_path(&self) -> anyhow::Result<PathBuf>;
}

impl UrlExt for Url {
    fn file_path(&self) -> anyhow::Result<PathBuf> {
        let pathbuf = unwrap!(self.to_file_path(), Err(_) => {
            return Err(anyhow!("error converting URI {} to PathBuf", self));
        });

        Ok(pathbuf)
    }
}

/// `tower_lsp_server` carries document URIs on the wire as `ls_types::Uri`,
/// a `fluent_uri`-backed type. The rest of this crate (in particular
/// `aether_path::FilePath`) standardizes on `url::Url`. Convert at the LSP
/// boundary so downstream code never has to deal with two URI types.
pub trait UriExt {
    fn to_url(&self) -> anyhow::Result<Url>;
}

impl UriExt for Uri {
    fn to_url(&self) -> anyhow::Result<Url> {
        Url::parse(self.as_str())
            .with_context(|| format!("error converting URI {} to URL", self.as_str()))
    }
}

/// The reverse of [`UriExt::to_url()`], for building outgoing LSP responses
/// that carry a `Uri` (e.g. `Location`, `WorkspaceEdit`).
pub trait UrlUriExt {
    fn to_uri(&self) -> anyhow::Result<Uri>;
}

impl UrlUriExt for Url {
    fn to_uri(&self) -> anyhow::Result<Uri> {
        self.as_str()
            .parse::<Uri>()
            .map_err(|err| anyhow!("error converting URL {self} to URI: {err}"))
    }
}
