//
// help.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use core::panic;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::help_comm::HelpBackendReply;
use amalthea::comm::help_comm::HelpBackendRequest;
use amalthea::comm::help_comm::ShowHelpTopicParams;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::iopub::IOPubMessage;
use ark::help::message::HelpEvent;
use ark::help::r_help::RHelp;
use ark::help_proxy;
use ark::r_task::r_task;
use ark_test::IOPubReceiverExt;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use harp::exec::RFunction;

struct TestRHelp {
    comm: CommSocket,
    iopub_rx: Receiver<IOPubMessage>,
    _help_event_tx: Sender<HelpEvent>,
}

impl TestRHelp {
    fn new(comm_id: String) -> Self {
        // Create a dummy iopub channel to receive responses.
        let (iopub_tx, iopub_rx) = crossbeam::channel::bounded::<IOPubMessage>(10);

        let comm = CommSocket::new(
            CommInitiator::FrontEnd,
            comm_id,
            String::from("positron.help"),
            iopub_tx,
        );
        // Start the help comm. It's important to save the help event sender so
        // that the help comm doesn't exit before we're done with it; allowing the
        // sender to be dropped signals the help comm to exit.
        let r_port = r_task(|| RHelp::r_start_or_reconnect_to_help_server().unwrap());
        let proxy_port = help_proxy::start(r_port).unwrap();
        let _help_event_tx = RHelp::start(comm.clone(), r_port, proxy_port).unwrap();

        Self {
            comm,
            iopub_rx,
            _help_event_tx,
        }
    }

    fn test_topic(&self, topic: &str, id: &str) {
        // Send a request for the help topic
        let request = HelpBackendRequest::ShowHelpTopic(ShowHelpTopicParams {
            topic: String::from(topic),
        });
        let data = serde_json::to_value(request).unwrap();
        let request_id = String::from(id);
        self.comm
            .incoming_tx
            .send(CommMsg::Rpc {
                id: request_id.clone(),
                parent_header: None,
                data,
            })
            .unwrap();

        // Wait for the response
        let response = self.iopub_rx.recv_comm_msg();
        match response {
            CommMsg::Rpc { id, data: val, .. } => {
                let response = serde_json::from_value::<HelpBackendReply>(val).unwrap();
                match response {
                    HelpBackendReply::ShowHelpTopicReply(found) => {
                        // Ensure we got a reply with an ID that matches the request
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
    let r_help = TestRHelp::new(String::from("test-help-comm-id"));

    r_help.test_topic("library", "help-test-id-1");
    r_help.test_topic("utils::find", "help-test-id-2");
    // Can come through this way if users request help while their cursor is on
    // an internal function
    r_help.test_topic("utils:::find", "help-test-id-3");

    // Figure out which port the R help server is running on (or would run on)
    let r_help_port = r_task(|| unsafe {
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
    let r_help = TestRHelp::new(String::from("test-help-comm-id-2"));

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
    assert_eq!(
        r_task(|| unsafe { harp::parse_eval_global("called").unwrap().to::<bool>() }).unwrap(),
        true,
    );
}
