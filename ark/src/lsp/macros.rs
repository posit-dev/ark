// 
// macros.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

pub trait IntoOption<T> {
    fn into_option(self) -> Option<T>;
}

impl<T, E> IntoOption<T> for Result<T, E> {
    fn into_option(self) -> Option<T> {
        self.ok()
    }
}

impl<T> IntoOption<T> for Option<T> {
    fn into_option(self) -> Option<T> {
        self
    }
}

pub fn _into_option<T>(object: impl IntoOption<T>) -> Option<T> {
    object.into_option()
}

macro_rules! unwrap {

    ($value: expr, $fail: expr) => {
        match crate::lsp::macros::_into_option($value) {
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
