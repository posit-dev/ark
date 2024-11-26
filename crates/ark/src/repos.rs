//
// repos.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;

#[derive(Debug)]
pub enum DefaultRepos {
    /// Do not set the repository automatically
    None,

    /// Set the repository automatically. This checks for `/etc/rstudio/repos.conf` on
    /// Unix-alikes); if found, it is used (as if were set as the `ConfFile`). If not, sets
    /// `cran.rstudio.com` as the CRAN repository
    Auto,

    /// Set the repository to the default CRAN repository, `cran.rstudio.com`
    RStudio,

    /// Use Posit's Public Package Manager; this is a Posit-hosted service hosts built binaries for
    /// many operating systems.
    PositPPM,

    /// Use the repositories specified in the given configuration file.
    ConfFile(PathBuf),
}
