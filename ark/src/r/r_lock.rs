// 
// r_lock.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

// NOTE: All execution of R code should first attempt to acquire
// this lock before execution. The Ark LSP's execution model allows
// arbitrary threads and tasks to communicate with the R session,
// and we mediate that through a global execution lock which must be
// held when interacting with R.
//
// One can either use the 'with_r_lock' function directly, e.g.
//
//     with_r_lock(&mut || { ... })
//
// Or, they can use the helper macro:
//
//     rlock! {
//         ...
//     }
//
// (which effectively expands into the above).

use lazy_static::lazy_static;
use parking_lot::ReentrantMutex;

macro_rules! rlock {

    ($($expr:tt)*) => {{
        let _lock = crate::r::r_lock::LOCK.lock();
        unsafe { $($expr)* }
    }}

}
pub(crate) use rlock;

pub fn with_r_lock<T, Callback: FnMut() -> T>(callback: &mut Callback) -> T {
    
    let result = {
        let _lock = LOCK.lock();
        callback()
    };

    return result;

}

lazy_static! {
    pub static ref LOCK: ReentrantMutex<()> = ReentrantMutex::new(());
}
