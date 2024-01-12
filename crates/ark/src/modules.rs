//
// modules.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::env;
use std::path::Path;

use harp::environment::R_ENVS;
use harp::exec::r_source_in;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_symbol;
use harp::utils::r_poke_option;
use libr::R_NilValue;
use libr::Rf_ScalarLogical;
use libr::Rf_asInteger;
use libr::SEXP;
use stdext::unwrap;

pub fn initialize(testing: bool) -> anyhow::Result<()> {
    // If we are `testing`, set the corresponding R level global option
    if testing {
        r_poke_option_ark_testing()
    }

    // Get the path to the 'modules' directory, adjacent to the executable file.
    // This is where we place the R source files in packaged releases.
    let exe_path = env::current_exe()?;
    let mut root = exe_path.parent().unwrap().join("modules");

    // If that path doesn't exist, we're probably running from source, so
    // look in the source tree (found via the 'CARGO_MANIFEST_DIR' environment
    // variable).
    if !root.exists() {
        let source = env!("CARGO_MANIFEST_DIR");
        root = Path::new(&source).join("src").join("modules").to_path_buf();
    }

    let positron_path = root.join("positron");
    let rstudio_path = root.join("rstudio");

    // Create the private Positron namespace.
    let namespace = RFunction::new("base", "new.env")
        .param("parent", R_ENVS.base)
        .call()?;

    // Load initial utils into the namespace
    let init_file = positron_path.join("init.R");
    r_source_in(init_file.to_str().unwrap(), namespace.sexp)?;

    // Load the positron namespace and exported functions
    walk_directory(&positron_path, "import_positron", namespace.sexp)?;

    // Load the rstudio namespace and exported functions
    walk_directory(&rstudio_path, "import_rstudio", namespace.sexp)?;

    return Ok(());
}

pub fn walk_directory(directory: &Path, fun: &str, env: SEXP) -> anyhow::Result<()> {
    log::info!("Loading modules from directory: {}", directory.display());
    let entries = std::fs::read_dir(directory)?;

    for entry in entries {
        let entry = unwrap!(
            entry,
            Err(err) => {
                log::error!("Can't load directory entry due to: {}", err);
                continue;
            }
        );

        let path = entry.path();

        if path.extension().is_some_and(|ext| ext == "R") {
            let path_string = path.display().to_string();
            RFunction::new("", fun)
                .param("path", path_string)
                .call_in(env)?;
        }
    }

    Ok(())
}

fn r_poke_option_ark_testing() {
    unsafe {
        let value = Rf_ScalarLogical(1);
        r_poke_option(r_symbol!("ark.testing"), value);
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_deep_sleep(secs: SEXP) -> anyhow::Result<SEXP> {
    let secs = Rf_asInteger(secs);
    let secs = std::time::Duration::from_secs(secs as u64);
    std::thread::sleep(secs);

    return Ok(R_NilValue);
}
