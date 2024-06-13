//
// help.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use core::panic;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::help_comm::HelpBackendReply;
use amalthea::comm::help_comm::HelpBackendRequest;
use amalthea::comm::help_comm::ShowHelpTopicParams;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use ark::help::r_help::RHelp;
use ark::help_proxy;
use ark::r_task::r_task;
use ark::test::r_test;
use harp::exec::RFunction;

/**
 * Basic test for the R help comm; requests help for a topic and ensures that we
 * get a reply.
 */
#[test]
fn test_help_comm() {
    r_test(|| {
        // Create the comm socket for the Help comm
        let comm = CommSocket::new(
            CommInitiator::FrontEnd,
            String::from("test-help-comm-id"),
            String::from("positron.help"),
        );

        let incoming_tx = comm.incoming_tx.clone();
        let outgoing_rx = comm.outgoing_rx.clone();

        // Start the help comm. It's important to save the help event sender so
        // that the help comm doesn't exit before we're done with it; allowing the
        // sender to be dropped signals the help comm to exit.
        let r_port = RHelp::r_start_or_reconnect_to_help_server().unwrap();
        let proxy_port = help_proxy::start(r_port).unwrap();
        let _help_event_tx = RHelp::start(comm, r_port, proxy_port).unwrap();

        // Utility function for testing `ShowHelpTopic` requests
        let test_topic = |topic: &str, id: &str| {
            // Send a request for the help topic
            let request = HelpBackendRequest::ShowHelpTopic(ShowHelpTopicParams {
                topic: String::from(topic),
            });
            let data = serde_json::to_value(request).unwrap();
            let request_id = String::from(id);
            incoming_tx
                .send(CommMsg::Rpc(request_id.clone(), data))
                .unwrap();

            // Wait for the response (up to 1 second; this should be fast!)
            let duration = std::time::Duration::from_secs(1);
            let response = outgoing_rx.recv_timeout(duration).unwrap();
            match response {
                CommMsg::Rpc(id, val) => {
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
        };

        test_topic("library", "help-test-id-1");
        test_topic("utils::find", "help-test-id-2");
        // Can come through this way if users request help while their cursor is on
        // an internal function
        test_topic("utils:::find", "help-test-id-3");

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
    })
}
