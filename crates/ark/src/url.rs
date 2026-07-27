//
// url.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use aether_path::FilePath;
use amalthea::wire::execute_request::CodeLocation;

/// Extract a canonical [`FilePath`] from a [`CodeLocation`].
pub fn file_path_from_code_location(loc: &CodeLocation) -> FilePath {
    FilePath::from_url(&loc.uri)
}

/// Ark-specific identity questions about a [`FilePath`].
pub trait FilePathExt {
    /// Whether this identifies an `ark://` virtual document (e.g. debugger
    /// vdocs showing foreign code the user can't edit).
    fn is_ark_virtual_doc(&self) -> bool;

    /// Whether this document should get diagnostics. Currently uses an
    /// exclude list: only `ark://` virtual documents are excluded.
    fn should_diagnose(&self) -> bool;
}

impl FilePathExt for FilePath {
    fn is_ark_virtual_doc(&self) -> bool {
        self.as_virtual()
            .is_some_and(|uri| uri.as_url().scheme() == "ark")
    }

    fn should_diagnose(&self) -> bool {
        !self.is_ark_virtual_doc()
    }
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;

    #[test]
    fn test_is_ark_virtual_doc() {
        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(FilePath::from_url(&ark_uri).is_ark_virtual_doc());

        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(!FilePath::from_url(&file_uri).is_ark_virtual_doc());
    }

    #[test]
    fn test_should_diagnose() {
        let file_uri = Url::parse("file:///home/user/test.R").unwrap();
        assert!(FilePath::from_url(&file_uri).should_diagnose());

        let git_uri = Url::parse("git:///home/user/test.R?ref=HEAD").unwrap();
        assert!(FilePath::from_url(&git_uri).should_diagnose());

        let ark_uri = Url::parse("ark://namespace/test.R").unwrap();
        assert!(!FilePath::from_url(&ark_uri).should_diagnose());
    }
}
