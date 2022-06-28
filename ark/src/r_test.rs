// 
// r_test.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::ffi::CString;
use std::process::Command;
use std::sync::Once;

use libR_sys::*;

static INIT: Once = Once::new();

pub fn start_r() {

    INIT.call_once(|| {
        // Compute R_HOME based on the version of R on the PATH
        let result = Command::new("R").arg("RHOME").output().expect("failed to run R");
        let home = String::from_utf8(result.stdout).expect("failed to read R RHOME output");
        std::env::set_var("R_HOME", home.trim());
        print!("Using R: {home}");

        // Build the argument list for Rf_initialize_R
        let arguments = ["--slave", "--no-save", "--no-restore"];
        let mut cargs = arguments.map(|value| {
            let result = CString::new(value).expect("error allocating C string");
            result.as_ptr() as *mut i8
        });

        unsafe {
            Rf_initialize_R(cargs.len() as i32, cargs.as_mut_ptr() as *mut *mut i8);
            setup_Rmainloop();
        }
    });
    
}
