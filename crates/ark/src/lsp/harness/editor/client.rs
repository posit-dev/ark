//! The simulated client's receiving end: records the server's notifications
//! and answers its requests. What it sends lives in `notifications`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use futures::SinkExt;
use futures::StreamExt;
use serde_json::json;
use serde_json::Value;
use tokio::task::JoinSet;
use tower::Service;
use tower_lsp_server::jsonrpc;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::Client;
use tower_lsp_server::ClientSocket;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::LspService;

/// Get a real `Client` without a live connection. `LspService::new` hands a
/// `Client` to its init closure; we capture it and drop the service. The
/// client's sends go nowhere, which is fine since the event paths under test
/// never use it. Use [`TestClient`] when a handler needs an answer.
#[cfg(test)]
pub(crate) fn test_client() -> Client {
    let (_service, _socket, client) = service();
    client
}

/// A live [`Client`] paired with a simulated editor that drains every outbound
/// message. It records notifications and answers requests so server handlers
/// waiting on the client can finish.
///
/// The peer returns configured values for `workspace/configuration` and
/// acknowledges `client/registerCapability` with an empty result. It rejects
/// and records any other request so the harness can fail on the caller's
/// thread. Panicking in the peer would only kill its task, surfacing indirectly
/// as a missing response.
///
/// Tests can update a value with [`Self::set_setting()`] before driving a
/// `didChangeConfiguration` notification.
pub(crate) struct TestClient {
    client: Client,
    #[cfg(test)]
    settings: Arc<Mutex<HashMap<String, Value>>>,
    requests: Arc<Mutex<Vec<Result<String, String>>>>,
    #[cfg(test)]
    notifications: Arc<Mutex<Vec<(String, Value)>>>,

    /// Aborts the peer on drop.
    _peer: JoinSet<()>,
}

impl TestClient {
    /// Build a client whose peer answers `settings`, given as `(section, value)`
    /// pairs.
    pub(crate) async fn new(settings: &[(&str, Value)]) -> Self {
        let (mut service, socket, client) = service();

        // `Client` suppresses outbound requests until the service has answered
        // an `initialize` request, so run one through it first. This is the
        // service's own state machine, unrelated to the `initialize` event the
        // tests send to the main loop.
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": { "capabilities": {} },
        });
        service
            .call(serde_json::from_value(initialize).unwrap())
            .await
            .unwrap();

        let settings = Arc::new(Mutex::new(
            settings
                .iter()
                .map(|(section, value)| ((*section).to_string(), value.clone()))
                .collect(),
        ));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let notifications = Arc::new(Mutex::new(Vec::new()));

        let mut peer = JoinSet::new();
        peer.spawn(answer_requests(
            socket,
            Arc::clone(&settings),
            Arc::clone(&requests),
            Arc::clone(&notifications),
        ));

        Self {
            client,
            #[cfg(test)]
            settings,
            requests,
            #[cfg(test)]
            notifications,
            _peer: peer,
        }
    }

    /// The client to hand to `GlobalState`. Every clone talks to the same peer.
    pub(crate) fn client(&self) -> Client {
        self.client.clone()
    }

    /// Change what the peer answers for `section`, as a user changing a setting
    /// does. Takes effect on the next `workspace/configuration` request.
    #[cfg(test)]
    pub(crate) fn set_setting(&self, section: &str, value: Value) {
        self.settings
            .lock()
            .unwrap()
            .insert(section.to_string(), value);
    }

    /// Request methods in arrival order. Supported methods are `Ok`, and
    /// unsupported methods are `Err`.
    pub(crate) fn answered_requests(&self) -> Vec<Result<String, String>> {
        self.requests.lock().unwrap().clone()
    }

    /// Methods and params of the notifications the server has sent, in order.
    #[cfg(test)]
    pub(crate) fn notifications(&self) -> Vec<(String, Value)> {
        self.notifications.lock().unwrap().clone()
    }

    /// Wait until the peer has read every message sent so far. The socket is
    /// FIFO, so the answer to a round-trip request arrives only after the
    /// earlier notifications have been recorded.
    ///
    /// A failed round-trip means the peer is gone and [`Self::notifications()`]
    /// may be incomplete, so this panics rather than letting a caller assert
    /// on partial state.
    #[cfg(test)]
    pub(crate) async fn flush(&self) {
        if let Err(err) = self.client.configuration(vec![]).await {
            panic!("The simulated editor did not answer a flush request: {err:?}");
        }
    }
}

/// Answer every request the server sends until it drops its [`Client`].
async fn answer_requests(
    socket: ClientSocket,
    settings: Arc<Mutex<HashMap<String, Value>>>,
    requests: Arc<Mutex<Vec<Result<String, String>>>>,
    notifications: Arc<Mutex<Vec<(String, Value)>>>,
) {
    let (mut incoming, mut outgoing) = socket.split();

    while let Some(request) = incoming.next().await {
        let (method, id, params) = request.into_parts();

        // A notification, such as `textDocument/publishDiagnostics`, carries no
        // id and gets no reply.
        let Some(id) = id else {
            notifications
                .lock()
                .unwrap()
                .push((method.to_string(), params.unwrap_or(Value::Null)));
            continue;
        };

        let (response, outcome) = match method.as_ref() {
            "workspace/configuration" => (
                jsonrpc::Response::from_ok(
                    id,
                    configuration_result(params.as_ref(), &settings.lock().unwrap()),
                ),
                Ok(method.to_string()),
            ),
            "client/registerCapability" => (
                jsonrpc::Response::from_ok(id, Value::Null),
                Ok(method.to_string()),
            ),
            method => (
                jsonrpc::Response::from_error(id, jsonrpc::Error::method_not_found()),
                Err(method.to_string()),
            ),
        };
        requests.lock().unwrap().push(outcome);

        if outgoing.send(response).await.is_err() {
            break;
        }
    }
}

/// One value per requested item, in the order asked, since `update_config()`
/// zips the response against the items it sent.
fn configuration_result(params: Option<&Value>, settings: &HashMap<String, Value>) -> Value {
    let Some(items) = params
        .and_then(|params| params.get("items"))
        .and_then(Value::as_array)
    else {
        return Value::Array(vec![]);
    };

    items
        .iter()
        .map(|item| {
            item.get("section")
                .and_then(Value::as_str)
                .and_then(|section| settings.get(section).cloned())
                .unwrap_or(Value::Null)
        })
        .collect()
}

/// A service, its loopback socket, and the server's `Client` handle from its
/// init closure. What the server sends through that handle arrives on the
/// socket, where the simulated editor reads it.
fn service() -> (LspService<Dummy>, ClientSocket, Client) {
    let captured = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&captured);
    let (service, socket) = LspService::new(move |client| {
        *sink.lock().unwrap() = Some(client);
        Dummy
    });

    // Bind first so the `MutexGuard` temporary drops at the `;`, not at the
    // end of the block.
    let client = captured.lock().unwrap().take();
    (service, socket, client.unwrap())
}

/// The `LanguageServer` the service dispatches to. Only `initialize` is ever
/// called, to open the service's outbound gate. Requests from the tests go
/// straight to `GlobalState::handle_event()` instead.
struct Dummy;

impl LanguageServer for Dummy {
    async fn initialize(
        &self,
        _: lsp_types::InitializeParams,
    ) -> jsonrpc::Result<lsp_types::InitializeResult> {
        Ok(lsp_types::InitializeResult::default())
    }

    async fn shutdown(&self) -> jsonrpc::Result<()> {
        Ok(())
    }
}
