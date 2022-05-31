// 
// macros.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

#![allow(unused_macros, unused_imports)]

macro_rules! expect {

    ($value:expr, $fail:expr) => {
        match $value {
            Ok(value) => value,
            Err(_error) => $fail,
        }
    }

}
pub(crate) use expect;

macro_rules! unwrap {

    ($value:expr, $fail:expr) => {
        match $value {
            Some(value) => value,
            None => $fail,
        }
    };

}
pub(crate) use unwrap;

macro_rules! backend_trace {

    ($self:expr, $($rest:expr),*) => {{
        let message = format!($($rest, )*);
        $self.client.log_message(tower_lsp::lsp_types::MessageType::INFO, message).await
    }};

}
pub(crate) use backend_trace;
