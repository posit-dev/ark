//
// local.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

#[macro_export]
macro_rules! local {
    ($($tokens:tt)*) => {
        (|| {
            $($tokens)*
        })()
    }
}

#[macro_export]
macro_rules! try_local {
    ($t:ty = {$($tokens:tt)*}) => {
        (|| -> Result<$t, anyhow::Error> {
            $($tokens)*
        })()
    }
}
