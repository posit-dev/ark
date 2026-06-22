//
// help.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use core::panic;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommEvent;
use amalthea::comm::help_comm::HelpBackendReply;
use amalthea::comm::help_comm::HelpBackendRequest;
use amalthea::comm::help_comm::ShowHelpTopicParams;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::socket::comm::CommOutgoingTx;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::comm_open::CommOpen;
use ark::comm_handler::CommHandler;
use ark::comm_handler::CommHandlerContext;
use ark::help::r_help::RHelp;
use ark::help_proxy;
use ark::r_task::r_task;
use ark_test::dummy_jupyter_header;
use ark_test::DummyArkFrontend;
use ark_test::IOPubReceiverExt;
use crossbeam::channel::bounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use harp::exec::RFunction;

struct TestRHelp {
    iopub_tx: Sender<IOPubMessage>,
    iopub_rx: Receiver<IOPubMessage>,
}

impl TestRHelp {
    fn new() -> Self {
        // Dummy iopub channel to receive the handler's outgoing messages.
        let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

        // Start the help server and proxy to mirror a real session. The RPC
        // handler is stateless, so we build a fresh `RHelp` and context per
        // request below.
        let r_port = r_task(|| RHelp::start_or_reconnect_to_help_server().unwrap());
        help_proxy::start(r_port).unwrap();

        Self { iopub_tx, iopub_rx }
    }

    fn test_topic(&self, topic: &str, id: &str) {
        let request = HelpBackendRequest::ShowHelpTopic(ShowHelpTopicParams {
            topic: String::from(topic),
        });
        let data = serde_json::to_value(request).unwrap();
        let request_id = String::from(id);
        let msg = CommMsg::Rpc {
            id: request_id.clone(),
            parent_header: dummy_jupyter_header(),
            data,
        };

        // The handler calls into R, so it must run on the R thread.
        let iopub_tx = self.iopub_tx.clone();
        r_task(move || {
            let comm_id = uuid::Uuid::new_v4().to_string();
            let outgoing_tx = CommOutgoingTx::new(comm_id, iopub_tx);
            let (comm_event_tx, _) = bounded::<CommEvent>(10);
            let ctx = CommHandlerContext::new(outgoing_tx, comm_event_tx);

            let mut handler = RHelp;
            handler.handle_msg(msg, &ctx);
        });

        let response = self.iopub_rx.recv_comm_msg();
        match response {
            CommMsg::Rpc { id, data: val, .. } => {
                let response = serde_json::from_value::<HelpBackendReply>(val).unwrap();
                match response {
                    HelpBackendReply::ShowHelpTopicReply(found) => {
                        assert!(found);
                        assert_eq!(id, request_id);
                    },
                }
            },
            _ => {
                panic!("Unexpected response from help comm: {:?}", response);
            },
        }
    }
}

/**
 * Basic test for the R help comm; requests help for a topic and ensures that we
 * get a reply.
 */
#[test]
fn test_help_comm() {
    let r_help = TestRHelp::new();

    r_help.test_topic("library", "help-test-id-1");
    r_help.test_topic("utils::find", "help-test-id-2");
    // Can come through this way if users request help while their cursor is on
    // an internal function
    r_help.test_topic("utils:::find", "help-test-id-3");

    // Figure out which port the R help server is running on (or would run on)
    let r_help_port = r_task(|| {
        RFunction::new_internal("tools", "httpdPort")
            .call()?
            .to::<u16>()
    })
    .unwrap();

    // This URL isn't in help format, so we don't expect it to be handled.
    let url = String::from("https://www.example.com");
    assert!(!RHelp::is_help_url(url.as_str(), r_help_port));

    // This one should be handled.
    let url = format!(
        "http://127.0.0.1:{}/library/base/html/plot.html",
        r_help_port
    );
    assert!(RHelp::is_help_url(url.as_str(), r_help_port));
}

#[test]
fn test_custom_help_handlers() {
    let r_help = TestRHelp::new();

    // Add a test help handler for an object
    r_task(|| {
        harp::parse_eval_global(
            r#"

        called <- FALSE
        .ark.register_method("ark_positron_help_get_handler", "foo", function(x) {
            function() {
                called <<- TRUE
            }
        })

        obj <- new.env()
        obj$hello <- structure(list(), class = "foo")
        "#,
        )
        .unwrap();
    });

    r_help.test_topic("obj$hello", "help-test-id-4");
    assert!(r_task(|| harp::parse_eval_global("called").unwrap().to::<bool>()).unwrap());
}

/// End-to-end test that a help URL browsed from R reaches the frontend as a
/// `show_help` event over the help comm.
///
/// This drives the kernel like a real session, exercising the path that the
/// unit tests above can't: opening the help comm registers the handler on the R
/// thread, and `browseURL()` of a help-server URL routes through our `browser`
/// option to `ps_browse_url()`, which sends a `show_help` event on the comm. The
/// event is delivered through the comm's stored context, the same mechanism that
/// fires reentrantly while a help topic is being printed.
#[test]
fn test_help_show_help_event() {
    let frontend = DummyArkFrontend::lock();

    // Open the help comm. This starts the R help server and proxy on the R
    // thread and registers the handler. A frontend-initiated comm open is
    // bracketed by a busy/idle pair on IOPub.
    let comm_id = uuid::Uuid::new_v4().to_string();
    frontend.send_shell(CommOpen {
        comm_id: comm_id.clone(),
        target_name: String::from("positron.help"),
        data: serde_json::json!({}),
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();

    // Requesting a help topic auto-prints it, which (with `help_type = "html"`)
    // calls `browseURL()` on the help-server URL. That routes through our
    // `browser` option to `ps_browse_url()`, is recognized as a help URL, and is
    // sent to the frontend as a `show_help` event, with the URL rewritten to
    // point at our help proxy.
    frontend.send_execute_request("?plot", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let msg = frontend.recv_iopub_comm_msg();
    assert_eq!(msg.comm_id, comm_id);
    assert_eq!(
        msg.data.get("method").and_then(|v| v.as_str()),
        Some("show_help")
    );
    assert_eq!(msg.data["params"]["kind"], "url");
    let content = msg.data["params"]["content"].as_str().unwrap();
    assert!(content.starts_with("http://127.0.0.1:"));
    assert!(content.contains("plot"));

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
