//
// r_version.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::mem::MaybeUninit;
use std::sync::Once;

use semver::Version;
use stdext::unwrap;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;

/// Determine the running R version
///
/// Determining the running R version from within ark
/// is complicated by the fact that there is no C function
/// in the R API that returns the version dynamically. To determine the
/// running R version, we call out to R once and store the result.
pub unsafe fn r_version() -> &'static Version {
    static INIT_R_VERSION: Once = Once::new();
    static mut R_VERSION: MaybeUninit<Version> = MaybeUninit::uninit();

    INIT_R_VERSION.call_once(|| {
        R_VERSION.write(unsafe { r_version_impl() });
    });

    unsafe { R_VERSION.assume_init_ref() }
}

unsafe fn r_version_impl() -> Version {
    let result = unwrap!(RFunction::new("base", "getRversion").call(), Err(error) => {
        log::error!("Failed in `getRversion()` due to: {error:?}.");
        return r_version_fallback();
    });

    let result = unwrap!(RFunction::new("base", "as.character").add(result).call(), Err(error) => {
        log::error!("Failed in `as.character()` due to: {error:?}.");
        return r_version_fallback();
    });

    let result = unwrap!(result.to::<String>(), Err(error) => {
        log::error!("Failed to convert version to string due to: {error:?}.");
        return r_version_fallback();
    });

    let version = unwrap!(semver::Version::parse(&result), Err(error) => {
        log::error!("Failed to parse version due to: {error:?}.");
        return r_version_fallback();
    });

    version
}

unsafe fn r_version_fallback() -> Version {
    semver::Version::new(0, 0, 0)
}
