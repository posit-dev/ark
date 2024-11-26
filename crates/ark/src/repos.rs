//
// repos.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::io::BufRead;
use std::path::PathBuf;

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::RObject;

use crate::modules::ARK_ENVS;

#[derive(Debug)]
pub enum DefaultRepos {
    /// Do not set the repository automatically
    None,

    /// Set the repository automatically. This checks for `repos.conf` in XDG locations; if found,
    /// it is used (as if were set as the `ConfFile`). If not, sets `cran.rstudio.com` as the CRAN
    /// repository
    Auto,

    /// Set the repository to the default CRAN repository, `cran.rstudio.com`
    RStudio,

    /// Use Posit's Public Package Manager; this is a Posit-hosted service that hosts built
    /// binaries for many operating systems.
    PositPPM,

    /// Use the repositories specified in the given configuration file.
    ConfFile(PathBuf),
}

pub fn apply_default_repos(repos: DefaultRepos) -> anyhow::Result<()> {
    match repos {
        DefaultRepos::None => {
            // This isn't the default, but it's a valid option
            log::debug!("Not applying any default repositories (none were set)");
            Ok(())
        },
        DefaultRepos::RStudio => {
            log::debug!("Setting default repositories to RStudio CRAN");
            let mut repos = HashMap::new();
            repos.insert("CRAN".to_string(), "https://cran.rstudio.com/".to_string());
            apply_repos(repos)
        },
        DefaultRepos::Auto => {
            // See if there's a repos file in the XDG directories
            if let Some(path) = find_repos_conf() {
                if let Err(e) = apply_repos_conf(path.clone()) {
                    // We failed to apply the repos file; log the error and use the
                    // RStudio defaults
                    log::error!("Error applying repos file {path:?}: {e}; using defaults");
                    apply_default_repos(DefaultRepos::RStudio)
                } else {
                    Ok(())
                }
            } else {
                // No repos file found; use the RStudio defaults
                apply_default_repos(DefaultRepos::RStudio)
            }
        },
        DefaultRepos::ConfFile(path) => {
            if path.exists() {
                if let Err(e) = apply_repos_conf(path.clone()) {
                    // We failed to apply the repos file; log the error and use defaults
                    log::error!(
                        "Error applying specified repos file: {path:?}: {e}; using defaults"
                    );
                    apply_default_repos(DefaultRepos::Auto)
                } else {
                    Ok(())
                }
            } else {
                log::warn!(
                    "Specified repos file {:?} does not exist; using defaults",
                    path
                );
                apply_default_repos(DefaultRepos::Auto)
            }
        },
        DefaultRepos::PositPPM => {
            log::info!("Setting default repositories to Posit's Public Package Manager");
            Ok(())
        },
    }
}

fn find_repos_conf_xdg(prefix: String) -> Option<PathBuf> {
    let xdg_dirs = match xdg::BaseDirectories::with_prefix(prefix.clone()) {
        Ok(xdg_dirs) => xdg_dirs,
        Err(e) => {
            log::error!("Error finding {prefix:?} XDG directories: {}", e);
            return None;
        },
    };
    xdg_dirs.find_config_file("repos.conf")
}

/// Finds a `repos.conf` file in the XDG configuration directories. Checks both RStudio and Ark
/// config folders.
fn find_repos_conf() -> Option<PathBuf> {
    if let Some(path) = find_repos_conf_xdg("rstudio".to_string()) {
        return Some(path);
    }
    if let Some(path) = find_repos_conf_xdg("ark".to_string()) {
        return Some(path);
    }
    None
}

/// Apply the given default repositories to the R session.
fn apply_repos(repos: HashMap<String, String>) -> anyhow::Result<()> {
    log::debug!("Applying default repositories: {:?}", repos);
    // Convert the HashMap to an R named character vector
    let named_repos = RObject::from(repos);

    // Call `apply_repo_defaults` to set the repos on the R side
    let mut call = RFunction::new("", "apply_repo_defaults");
    call.add(named_repos);
    match call.call_in(ARK_ENVS.positron_ns) {
        Ok(_) => Ok(()),
        Err(e) => Err(anyhow::anyhow!(
            "Error applying default repositories: {}",
            e
        )),
    }
}

/// Apply the repos configuration file at the given path.
///
/// The repos configuration file is a simple INI-style configuration file styled after RStudio's
/// /etc/rstudio/repos.conf. It is expected to consist of repository names and URLs, one per line,
/// in the format `name=url`. Empty lines or lines beginning with `#` (comments) are ignored.
///
/// Arguments:
/// - `path`: The path to the repos configuration file.
///
/// Returns:
///
/// `Ok(())` if the file was successfully read and applied, or an error if there was a
/// problem.
pub fn apply_repos_conf(path: PathBuf) -> anyhow::Result<()> {
    log::info!("Using repos file at {:?}", path);
    // Read the repos file
    let file = std::fs::File::open(&path)?;
    let reader = std::io::BufReader::new(file);
    let mut repos = std::collections::HashMap::<String, String>::new();
    for line in reader.lines() {
        let line = line?;
        // Ignore the line if it's only whitespace or starts with a comment
        if line.trim().is_empty() || line.trim().starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('=').collect();
        if parts.len() != 2 {
            log::debug!("Skipping invalid line in repos file: {}", line);
            continue;
        }
        let repo_name = parts[0].trim();
        let repo_url = parts[1].trim();
        repos.insert(repo_name.to_string(), repo_url.to_string());
    }
    apply_repos(repos)
}
