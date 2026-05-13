//
// version.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use harp::object::RObject;
use itertools::Itertools;
use libr::SEXP;
use oak_package_metadata::description::Description;

pub const MIN_R_MAJOR: u32 = 4;
pub const MIN_R_MINOR: u32 = 2;

pub struct RVersion {
    // Major version of the R installation
    pub major: u32,

    // Minor version of the R installation
    pub minor: u32,

    // Patch version of the R installation
    pub patch: u32,
}

impl RVersion {
    pub fn is_supported(&self) -> bool {
        self.major > MIN_R_MAJOR || (self.major == MIN_R_MAJOR && self.minor >= MIN_R_MINOR)
    }
}

pub fn from_r_home(r_home: &Path) -> anyhow::Result<RVersion> {
    let path = r_home.join("library").join("base").join("DESCRIPTION");

    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read R version from {}", path.display()))?;

    let description = Description::parse(&contents)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    parse_version_string(&description.version).with_context(|| {
        format!(
            "Failed to parse R version `{}` from {}",
            description.version,
            path.display()
        )
    })
}

fn parse_version_string(s: &str) -> anyhow::Result<RVersion> {
    let parts = s.trim().split('.').map(|x| x.parse::<u32>());

    if let Some((Ok(major), Ok(minor), Ok(patch))) = parts.collect_tuple() {
        Ok(RVersion {
            major,
            minor,
            patch,
        })
    } else {
        Err(anyhow::anyhow!("expected `major.minor.patch`"))
    }
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_ark_version() -> anyhow::Result<SEXP> {
    let mut info = HashMap::<String, String>::new();
    // Set the version info in the map
    info.insert(String::from("version"), String::from(crate::BUILD_VERSION));

    // Add the current commit hash and branch; these are set by the build script (build.rs)
    info.insert(String::from("commit"), String::from(env!("BUILD_GIT_HASH")));
    info.insert(
        String::from("branch"),
        String::from(env!("BUILD_GIT_BRANCH")),
    );

    // Add the build date; this is also set by the build script
    info.insert(String::from("date"), String::from(env!("BUILD_DATE")));

    // Add the path to the kernel
    let path = env::current_exe().unwrap_or_else(|_| PathBuf::from("<unknown>"));
    info.insert(String::from("path"), path.to_string_lossy().into_owned());

    // Insert the flavor (debug or release)
    #[cfg(debug_assertions)]
    info.insert(String::from("flavor"), String::from("debug"));
    #[cfg(not(debug_assertions))]
    info.insert(String::from("flavor"), String::from("release"));

    let result = RObject::from(info);
    Ok(result.sexp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_reject_low_major() {
        let version = RVersion {
            major: 3,
            minor: 9,
            patch: 0,
        };
        assert!(!version.is_supported());
    }

    #[test]
    fn test_version_reject_low_minor() {
        let version = RVersion {
            major: 4,
            minor: 1,
            patch: 0,
        };
        assert!(!version.is_supported());
    }

    #[test]
    fn test_version_accept_exact() {
        let version = RVersion {
            major: 4,
            minor: 2,
            patch: 0,
        };
        assert!(version.is_supported());
    }

    #[test]
    fn test_version_accept_high_minor() {
        let version = RVersion {
            major: 4,
            minor: 4,
            patch: 0,
        };
        assert!(version.is_supported());
    }

    #[test]
    fn test_version_accept_high_major() {
        let version = RVersion {
            major: 5,
            minor: 0,
            patch: 0,
        };
        assert!(version.is_supported());
    }

    #[test]
    fn test_parse_version_string_basic() {
        let version = parse_version_string("4.5.1").unwrap();
        assert_eq!(version.major, 4);
        assert_eq!(version.minor, 5);
        assert_eq!(version.patch, 1);
    }

    #[test]
    fn test_parse_version_string_trims_whitespace() {
        let version = parse_version_string("  4.5.1\n").unwrap();
        assert_eq!(version.major, 4);
        assert_eq!(version.minor, 5);
        assert_eq!(version.patch, 1);
    }

    #[test]
    fn test_parse_version_string_too_few_components() {
        assert!(parse_version_string("4.5").is_err());
    }

    #[test]
    fn test_parse_version_string_too_many_components() {
        assert!(parse_version_string("4.5.1.2").is_err());
    }

    #[test]
    fn test_parse_version_string_non_numeric() {
        assert!(parse_version_string("4.5.x").is_err());
    }

    #[test]
    fn test_parse_version_string_empty() {
        assert!(parse_version_string("").is_err());
    }
}
