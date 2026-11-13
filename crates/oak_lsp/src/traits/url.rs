//
// url.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use aether_path::FilePath;
use anyhow::*;
use tower_lsp_server::ls_types::Uri;
use url::Url;

/// `tower_lsp_server` carries document URIs on the wire as `ls_types::Uri`,
/// a `fluent_uri`-backed type. Convert at the LSP boundary so downstream code
/// never has to deal with two URI types.
pub trait UriExt {
    /// The document identity for this URI.
    ///
    /// Not named `to_file_path()` because of a conflict with `Uri::to_file_path()`.
    fn to_document_path(&self) -> anyhow::Result<FilePath>;

    /// The URI as a [`Url`]. Prefer [`Self::to_document_path()`] for identity.
    /// This is for the few places that need URL structure on the way to
    /// something else, such as deriving an [`aether_path::AbsPathBuf`] for a
    /// workspace folder.
    fn to_url(&self) -> anyhow::Result<Url>;
}

impl UriExt for Uri {
    fn to_document_path(&self) -> anyhow::Result<FilePath> {
        Ok(FilePath::from_url(&self.to_url()?))
    }

    fn to_url(&self) -> anyhow::Result<Url> {
        Url::parse(self.as_str())
            .with_context(|| format!("error converting URI {} to URL", self.as_str()))
    }
}

/// The reverse of [`UriExt::to_url()`], for building outgoing LSP responses
/// that carry a `Uri` (e.g. `Location`, `WorkspaceEdit`).
pub trait UrlExt {
    fn to_uri(&self) -> anyhow::Result<Uri>;
}

impl UrlExt for Url {
    fn to_uri(&self) -> anyhow::Result<Uri> {
        self.as_str()
            .parse::<Uri>()
            .map_err(|err| anyhow!("error converting URL {self} to URI: {err}"))
    }
}
