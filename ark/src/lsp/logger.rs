// 
// logger.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::sync::Mutex;
use lazy_static::lazy_static;

use tower_lsp::Client;
use tower_lsp::lsp_types::MessageType;

#[derive(Default)]
pub(crate) struct Logger {
    messages: Vec<String>,
}

#[doc(hidden)]
pub(crate) async fn flush(client: &Client) {
    
    let mut messages = Vec::new();

    if let Ok(mut logger) = LOGGER.lock() {
        messages = logger.messages.clone();
        logger.messages.clear();
    }
    
    for message in messages {
        client.log_message(MessageType::INFO, message).await;
    }

}

#[doc(hidden)]
pub(crate) fn append(message: String) {
    if let Ok(mut logger) = LOGGER.lock() {
        logger.messages.push(message);
    }
}

macro_rules! log_push {

    ($($rest:expr),*) => {{
        let message = format!($($rest, )*);
        crate::lsp::logger::append(message);
    }};

}
pub(crate) use log_push;

lazy_static! {
    static ref LOGGER : Mutex<Logger> = Mutex::new(Logger::default());
}
