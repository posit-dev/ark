//
// url.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use aether_url::UrlId;
use amalthea::wire::execute_request::CodeLocation;
use url::Url;

/// Extract a canonical [`UrlId`] from a [`CodeLocation`].
pub fn url_id_from_code_location(loc: &CodeLocation) -> UrlId {
    UrlId::from_url(loc.uri.clone())
}

/// Extended URL utilities.
///
/// These operate on raw `Url` values and don't require canonicalization.
/// For identity-sensitive operations (HashMap keys, breakpoint matching),
/// use [`UrlId`] instead.
pub struct ExtUrl;

impl ExtUrl {
    /// Whether this URI should be indexed. Currently uses an exclude list:
    /// only `ark://` virtual documents are excluded since they show foreign
    /// code the user can't edit.
    pub fn is_indexable(uri: &Url) -> bool {
        !Self::is_ark_virtual_doc(uri)
    }

    /// Whether this URI should get diagnostics. Currently uses the same
    /// exclude list as [`Self::is_indexable`] but kept separate so the
    /// criteria can diverge independently.
    pub fn should_diagnose(uri: &Url) -> bool {
        !Self::is_ark_virtual_doc(uri)
    }

    /// Whether this URI points to an `ark://` virtual document (e.g. debugger
    /// vdocs showing foreign code).
    pub fn is_ark_virtual_doc(uri: &Url) -> bool {
        uri.scheme() == "ark"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ark_virtual_doc() {
        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(ExtUrl::is_ark_virtual_doc(&ark_uri));

        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(!ExtUrl::is_ark_virtual_doc(&file_uri));
    }

    #[test]
    fn test_is_indexable() {
        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(ExtUrl::is_indexable(&file_uri));

        let git_uri = Url::parse("git:///home/user/test.R?ref=HEAD").unwrap();
        assert!(ExtUrl::is_indexable(&git_uri));

        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(!ExtUrl::is_indexable(&ark_uri));
    }

    #[test]
    fn test_should_diagnose() {
        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(ExtUrl::should_diagnose(&file_uri));

        let git_uri = Url::parse("git:///home/user/test.R?ref=HEAD").unwrap();
        assert!(ExtUrl::should_diagnose(&git_uri));

        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(!ExtUrl::should_diagnose(&ark_uri));
    }
}
