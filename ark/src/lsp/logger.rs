// 
// logger.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use tower_lsp::lsp_types::MessageType;

use crate::lsp::backend::Backend;

pub(crate) struct Logger {
    messages: Vec<String>,
}

impl Logger {

    pub(crate) fn append(&mut self, message: &str) {
        self.messages.push(message.to_string());
    }

    pub(crate) async fn flush(&mut self, backend: &Backend) {
        
        for message in &self.messages {
            backend.client.log_message(MessageType::INFO, message).await;
        }

        self.messages.clear();
    }

}

pub(crate) static mut LOGGER : Logger = Logger { messages: vec![] };

macro_rules! log_push {

    ($($rest:expr),*) => {{
        let message = format!($($rest, )*);
        unsafe { crate::lsp::logger::LOGGER.append(message.as_str()) };
    }};

}
pub(crate) use log_push;

macro_rules! log_flush {

    ($backend:expr) => {{
        unsafe { crate::lsp::logger::LOGGER.flush($backend).await };
    }};

}
pub(crate) use log_flush;
