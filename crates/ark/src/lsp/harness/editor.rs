//! The simulated editor on the other end of an `LspSession`, split by message
//! direction: `notifications` holds what the editor sends to the server, and
//! `client` receives and answers what the server sends back.

pub(crate) mod client;
pub(crate) mod notifications;
