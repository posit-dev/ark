//
// repos.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::fs::File;
use std::io::BufRead;
#[cfg(target_os = "linux")]
use std::io::BufReader;
use std::path::PathBuf;

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::RObject;

use crate::modules::ARK_ENVS;

// Constants for repository URLs
const GENERIC_P3M_REPO: &str = "https://packagemanager.posit.co/cran/latest";

#[derive(Debug)]
pub enum DefaultRepos {
    /// Do not set the repository at all (don't touch the `repos` option)
    None,

    /// Set the repository automatically. This checks for `repos.conf` in user and system XDG
    /// locations; if found, it is used (as if were set as the `ConfFile`). If not, sets
    /// `cran.rstudio.com` as the CRAN repository
    ///
    /// This is the default unless otherwise specified.
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
            // Use the RStudio CRAN repository
            log::debug!("Setting default repositories to RStudio CRAN");
            let mut repos = HashMap::new();
            repos.insert("CRAN".to_string(), "https://cran.rstudio.com/".to_string());
            apply_repos(repos)
        },
        DefaultRepos::Auto => {
            // The user didn't specify any default repositories. See if there's a repos file in the
            // XDG directories.
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
                    // We failed to apply the repos file; log the error and use defaults so we
                    // still have a good shot at a working CRAN mirror
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
            let mut repos = HashMap::new();
            repos.insert("CRAN".to_string(), get_p3m_binary_package_repo());
            apply_repos(repos)
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

#[cfg(target_os = "linux")]
fn get_p3m_linux_repo(linux_name: String) -> String {
    // The following Linux names have 1:1 mappings to a P3M repository URL
    let repo_names = [
        String::from("bookworm"),
        String::from("bullseye"),
        String::from("focal"),
        String::from("jammy"),
        String::from("noble"),
        String::from("opensuse155"),
        String::from("opensuse156"),
        String::from("rhel9"),
    ];

    // First check for an empty name, and default to the generic P3M repo in that case.
    // Then handle Linux names with a 1:1 mapping to a P3M repo.
    // Then handle the special cases which map to different P3M repos.
    // Otherwise, default to the generic P3M repo.
    if linux_name.is_empty() {
        return GENERIC_P3M_REPO.to_string();
    } else if repo_names.contains(&linux_name) {
        return format!(
            "https://packagemanager.posit.co/cran/__linux__/{}/latest",
            linux_name
        );
    } else if linux_name == "rhel8" {
        return "https://packagemanager.posit.co/cran/__linux__/centos8/latest".to_string();
    } else if linux_name == "sles155" {
        return "https://packagemanager.posit.co/cran/__linux__/opensuse155/latest".to_string();
    } else if linux_name == "sles156" {
        return "https://packagemanager.posit.co/cran/__linux__/opensuse156/latest".to_string();
    } else {
        return GENERIC_P3M_REPO.to_string();
    }
}

#[cfg(target_os = "linux")]
fn get_p3m_linux_codename(id: String, version: String, version_codename: String) -> String {
    // For Debian and Ubuntu, we can just use the codename
    if id == "debian" || id == "ubuntu" {
        return version_codename.to_string();
    } else if id == "rhel" {
        // For RHEL, we use the id and major version number
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() > 1 {
            return format!("{}{}", id, parts[0]);
        } else {
            return format!("{}{}", id, version);
        }
    } else if id == "sles" || id.starts_with("opensuse") {
        // For sles and opensuse we use the id and major and minor version number
        // stripped of any dot separator
        let distro_id = if id == "sles" { "sles" } else { "opensuse" };

        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() > 1 {
            return format!("{}{}{}", distro_id, parts[0], parts[1]);
        } else {
            return format!("{}{}", distro_id, version);
        }
    } else {
        return String::new();
    }
}

fn get_p3m_binary_package_repo() -> String {
    #[cfg(target_os = "linux")]
    {
        // For Linux, we want a distro-specific URL if possible
        // Read the /etc/os-release file to determine the Linux distribution info
        let mut id = String::new();
        let mut version = String::new();
        let mut version_codename = String::new();
        let version_codename_key = "VERSION_CODENAME=";
        let id_key = "ID=";
        let version_id_key = "VERSION_ID=";
        if let Ok(file) = File::open("/etc/os-release") {
            let reader = BufReader::new(file);

            for line in reader.lines().flatten() {
                if version_codename.is_empty() && line.starts_with(version_codename_key) {
                    version_codename = line[version_codename_key.len()..].to_string();
                } else if id.is_empty() && line.starts_with(id_key) {
                    id = line[id_key.len()..].to_string();
                } else if version.is_empty() && line.starts_with(version_id_key) {
                    version = line[version_id_key.len()..].to_string();
                }
            }
        }

        get_p3m_linux_repo(get_p3m_linux_codename(id, version, version_codename))
    }

    #[cfg(not(target_os = "linux"))]
    {
        // For non-Linux, we can use the generic P3M URL
        GENERIC_P3M_REPO.to_string()
    }
}
