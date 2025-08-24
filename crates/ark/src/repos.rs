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

#[derive(Debug, PartialEq)]
pub enum DefaultRepos {
    /// Do not set the repository at all (don't touch the `repos` option)
    None,

    /// Set the repository automatically. This checks for `repos.conf` in user and system XDG
    /// locations for Unix-alikes; if found, it is used (as if were set as the `ConfFile`). If not,
    /// sets `cran.rstudio.com` as the CRAN repository
    ///
    /// This is the default unless otherwise specified.
    Auto,

    /// Set the repository to the default CRAN repository, `cran.rstudio.com`
    RStudio,

    /// Use a Posit Package Manager instance with this URL. When the URL is
    /// `None`, default to the latest CRAN repository on Posit Public Package
    /// Manager, a Posit-hosted service that hosts built binaries for many
    /// operating systems.
    PositPackageManager(Option<url::Url>),

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
        DefaultRepos::Auto => apply_default_repos_auto(),
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
        DefaultRepos::PositPackageManager(None) => {
            log::info!("Setting default repositories to Posit's Public Package Manager");
            let mut repos = HashMap::new();
            repos.insert("CRAN".to_string(), get_ppm_binary_package_repo(None));
            apply_repos(repos)
        },
        DefaultRepos::PositPackageManager(Some(url)) => {
            log::info!(
                "Setting default repositories to custom Package Manager repo: {}",
                url
            );
            let mut repos = HashMap::new();
            repos.insert("CRAN".to_string(), get_ppm_binary_package_repo(Some(url)));
            apply_repos(repos)
        },
    }
}

/// The automatic default repository setting. This checks for a `repos.conf` file in the XDG config
/// directories; if found, it is used. If not, the RStudio CRAN mirror is used.
///
/// We only use this variant on Unix-like systems, as the `xdg` crate is Unix-only.
#[cfg(unix)]
fn apply_default_repos_auto() -> anyhow::Result<()> {
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
}

/// On Windows, we just use the RStudio CRAN mirror as the default.
#[cfg(not(unix))]
fn apply_default_repos_auto() -> anyhow::Result<()> {
    apply_default_repos(DefaultRepos::RStudio)
}

#[cfg(unix)]
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
#[cfg(unix)]
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

/// Checks the Linux distribution name and version to determine the appropriate P3M repository URL.
#[cfg(target_os = "linux")]
fn get_ppm_linux_repo(repo_url: Option<url::Url>, linux_name: String) -> anyhow::Result<String> {
    let generic_url = match repo_url {
        Some(url) => url,
        None => url::Url::parse(GENERIC_P3M_REPO).unwrap(),
    };

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

    // Handle special cases which map to different P3M repos.
    let distro = if repo_names.contains(&linux_name) {
        &linux_name
    } else if linux_name == "rhel8" {
        "centos8"
    } else if linux_name == "sles155" {
        "opensuse155"
    } else if linux_name == "sles156" {
        "opensuse156"
    } else {
        return Ok(generic_url.to_string());
    };

    let mut distro_url = generic_url.clone();
    if let Some(segments) = distro_url.path_segments() {
        let parts: Vec<&str> = segments.collect();
        if parts.len() == 2 {
            distro_url.set_path(&format!("{}/__linux__/{}/{}", parts[0], distro, parts[1]));
            return Ok(distro_url.to_string());
        }
    }
    anyhow::bail!("Invalid Package Manager repository URL: {}", distro_url);
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

fn get_ppm_binary_package_repo(repo_url: Option<url::Url>) -> String {
    let generic_url = match repo_url {
        Some(ref url) => url.clone().to_string(),
        None => GENERIC_P3M_REPO.to_string(),
    };

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
        } else {
            log::error!(
                "Error opening /etc/os-release, falling back to generic URL: {generic_url}",
            );
            return generic_url;
        }

        let codename = get_p3m_linux_codename(id, version, version_codename);
        match get_ppm_linux_repo(repo_url, codename) {
            Ok(url) => url,
            Err(e) => {
                log::error!(
                    "Error determining Linux binary repository URL, falling back to generic URL '{generic_url}': {e}",
                );
                generic_url
            },
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        // For non-Linux, we can use the generic URL
        generic_url
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn test_get_ppm_linux_repo() {
        let test_cases = vec![
            // Supported distros.
            (
                "bookworm",
                "https://packagemanager.posit.co/cran/__linux__/bookworm/latest",
            ),
            (
                "bullseye",
                "https://packagemanager.posit.co/cran/__linux__/bullseye/latest",
            ),
            (
                "focal",
                "https://packagemanager.posit.co/cran/__linux__/focal/latest",
            ),
            (
                "jammy",
                "https://packagemanager.posit.co/cran/__linux__/jammy/latest",
            ),
            (
                "noble",
                "https://packagemanager.posit.co/cran/__linux__/noble/latest",
            ),
            (
                "opensuse155",
                "https://packagemanager.posit.co/cran/__linux__/opensuse155/latest",
            ),
            (
                "opensuse156",
                "https://packagemanager.posit.co/cran/__linux__/opensuse156/latest",
            ),
            (
                "rhel9",
                "https://packagemanager.posit.co/cran/__linux__/rhel9/latest",
            ),
            // Special cases.
            (
                "rhel8",
                "https://packagemanager.posit.co/cran/__linux__/centos8/latest",
            ),
            (
                "sles155",
                "https://packagemanager.posit.co/cran/__linux__/opensuse155/latest",
            ),
            (
                "sles156",
                "https://packagemanager.posit.co/cran/__linux__/opensuse156/latest",
            ),
            // Unsupported distros fall back to the generic URL.
            ("centos7", GENERIC_P3M_REPO),
            ("arch", GENERIC_P3M_REPO),
            ("", GENERIC_P3M_REPO),
        ];

        for (distro, expected) in test_cases {
            let result = get_ppm_linux_repo(None, distro.to_string()).unwrap();
            assert_eq!(result, expected);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_get_custom_ppm_linux_repo() {
        let test_cases = vec![
            (
                "jammy",
                "https://ppm.internal/approved/__linux__/jammy/2025-03-02",
            ),
            (
                "rhel8",
                "https://ppm.internal/approved/__linux__/centos8/2025-03-02",
            ),
            ("arch", "https://ppm.internal/approved/2025-03-02"),
            ("", "https://ppm.internal/approved/2025-03-02"),
        ];

        let custom_url = url::Url::parse("https://ppm.internal/approved/2025-03-02").unwrap();
        for (distro, expected) in test_cases {
            let result = get_ppm_linux_repo(Some(custom_url.clone()), distro.to_string()).unwrap();
            assert_eq!(result, expected);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_invalid_ppm_url() {
        let custom_url = url::Url::parse("https://ppm.internal/not/a/repo").unwrap();
        let result = get_ppm_linux_repo(Some(custom_url), "jammy".to_string());
        assert!(result.is_err());
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_custom_ppm_url_for_non_linux() {
        let custom_url = url::Url::parse("https://ppm.internal/approved/2025-03-02").unwrap();
        let result = get_ppm_binary_package_repo(Some(custom_url));
        assert_eq!(result, "https://ppm.internal/approved/2025-03-02");
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn test_generic_ppm_url_for_non_linux() {
        assert_eq!(get_ppm_binary_package_repo(None), GENERIC_P3M_REPO);
    }
}
