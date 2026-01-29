//
// r_env.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::env;
use std::path::PathBuf;
use std::process::Command;

/// Find R_HOME without loading R shared libraries.
/// Uses R CMD config or environment variables.
pub fn find_r_home() -> Option<PathBuf> {
    // First check environment variable
    if let Ok(r_home) = env::var("R_HOME") {
        let path = PathBuf::from(&r_home);
        if path.exists() {
            return Some(path);
        }
    }

    // Try R CMD config
    if let Ok(output) = Command::new("R").args(["CMD", "config", "R_HOME"]).output() {
        if output.status.success() {
            let r_home = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let path = PathBuf::from(&r_home);
            if path.exists() {
                return Some(path);
            }
        }
    }

    // Platform-specific fallbacks
    #[cfg(target_os = "macos")]
    {
        let paths = [
            "/Library/Frameworks/R.framework/Resources",
            "/opt/homebrew/Cellar/r",
            "/usr/local/Cellar/r",
        ];
        for p in paths {
            let path = PathBuf::from(p);
            if path.exists() {
                // For Homebrew, find the actual version directory
                if p.contains("Cellar") {
                    if let Ok(entries) = std::fs::read_dir(&path) {
                        for entry in entries.flatten() {
                            let version_path = entry.path().join("lib/R");
                            if version_path.exists() {
                                return Some(version_path);
                            }
                        }
                    }
                } else {
                    return Some(path);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let paths = ["/usr/lib/R", "/usr/lib64/R", "/usr/local/lib/R"];
        for p in paths {
            let path = PathBuf::from(p);
            if path.exists() {
                return Some(path);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Check common Windows R installation paths
        if let Ok(program_files) = env::var("ProgramFiles") {
            let r_dir = PathBuf::from(&program_files).join("R");
            if r_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&r_dir) {
                    // Find the latest R version
                    let mut versions: Vec<_> = entries.flatten().collect();
                    versions.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
                    if let Some(latest) = versions.first() {
                        return Some(latest.path());
                    }
                }
            }
        }
    }

    None
}

/// Find R library paths where packages are installed.
pub fn find_library_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Try R CMD config to get library paths
    if let Ok(output) = Command::new("R")
        .args(["--vanilla", "--slave", "-e", "cat(.libPaths(), sep='\\n')"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let path = PathBuf::from(line.trim());
                if path.exists() && !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }

    // If we couldn't get paths from R, use defaults
    if paths.is_empty() {
        if let Some(r_home) = find_r_home() {
            let library = r_home.join("library");
            if library.exists() {
                paths.push(library);
            }
        }

        // User library
        if let Some(user_lib) = user_library_path() {
            if user_lib.exists() {
                paths.push(user_lib);
            }
        }
    }

    paths
}

/// Get the user's R library path
fn user_library_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        if let Ok(xdg) = xdg::BaseDirectories::new() {
            // R typically uses ~/R/x86_64-pc-linux-gnu-library/X.Y on Linux
            // and ~/Library/R/x86_64/X.Y/library on macOS
            let home = xdg.get_data_home().parent()?.to_path_buf();

            #[cfg(target_os = "macos")]
            {
                let r_lib = home.join("Library/R");
                if r_lib.exists() {
                    // Find architecture and version subdirectories
                    if let Ok(entries) = std::fs::read_dir(&r_lib) {
                        for arch_entry in entries.flatten() {
                            if let Ok(versions) = std::fs::read_dir(arch_entry.path()) {
                                for ver_entry in versions.flatten() {
                                    let lib_path = ver_entry.path().join("library");
                                    if lib_path.exists() {
                                        return Some(lib_path);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            #[cfg(target_os = "linux")]
            {
                let r_lib = home.join("R");
                if r_lib.exists() {
                    if let Ok(entries) = std::fs::read_dir(&r_lib) {
                        for entry in entries.flatten() {
                            if let Ok(versions) = std::fs::read_dir(entry.path()) {
                                for ver_entry in versions.flatten() {
                                    if ver_entry.path().is_dir() {
                                        return Some(ver_entry.path());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if let Ok(docs) = env::var("USERPROFILE") {
            let r_lib = PathBuf::from(docs).join("Documents/R/win-library");
            if r_lib.exists() {
                if let Ok(entries) = std::fs::read_dir(&r_lib) {
                    let mut versions: Vec<_> = entries.flatten().collect();
                    versions.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
                    if let Some(latest) = versions.first() {
                        return Some(latest.path());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_r_home() {
        // This test will pass if R is installed
        let r_home = find_r_home();
        if let Some(path) = &r_home {
            assert!(path.exists());
            // R_HOME should contain a library directory
            assert!(path.join("library").exists() || path.join("lib").exists());
        }
    }

    #[test]
    fn test_find_library_paths() {
        let paths = find_library_paths();
        // Should find at least one library path if R is installed
        for path in &paths {
            assert!(path.exists());
        }
    }
}
