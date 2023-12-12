//
// r_version.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::env;
use std::mem::MaybeUninit;
use std::sync::Once;

use semver::Version;
use stdext::unwrap;

/// Determine the running R version
///
/// Determining the running R version from within ark
/// is complicated by the fact that there is no C _function_
/// in the R API that returns the version dynamically. Instead,
/// there is an `R_VERSION` macro, but this reflects the R version
/// at libR-sys build time, not at runtime. To determine the running
/// R version, we look up the `ARK_R_VERSION` environment variable once and
/// store the result.
///
/// This function is used during initialization routines, so it must not use
/// harp utilities that call out to R.
pub fn r_version() -> &'static Version {
    static INIT_R_VERSION: Once = Once::new();
    static mut R_VERSION: MaybeUninit<Version> = MaybeUninit::uninit();

    INIT_R_VERSION.call_once(|| unsafe {
        R_VERSION.write(r_version_impl());
    });

    unsafe { R_VERSION.assume_init_ref() }
}

fn r_version_impl() -> Version {
    let version = unwrap!(env::var("ARK_R_VERSION"), Err(err) => {
        log::error!("Failed to get `ARK_R_VERSION` environment variable due to: {err:?}.");
        return r_version_fallback();
    });

    let version = unwrap!(semver::Version::parse(&version), Err(err) => {
        log::error!("Failed to parse version due to: {err:?}.");
        return r_version_fallback();
    });

    version
}

fn r_version_fallback() -> Version {
    semver::Version::new(0, 0, 0)
}
