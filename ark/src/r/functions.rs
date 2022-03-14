use libc::{c_int, c_void};

#[link(name = "R", kind = "dylib")]
extern "C" {
    /// Initialize R
    pub fn Rf_initialize_R(ac: c_int, av: *mut c_void) -> i32;

    /// Run the R main execution loop (does not return)
    pub fn Rf_mainloop();
}
