//
// help.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use core::panic;
use std::net::TcpStream;
use std::time::Duration;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommEvent;
use amalthea::comm::help_comm::GetHelpTopicsParams;
use amalthea::comm::help_comm::HelpBackendReply;
use amalthea::comm::help_comm::HelpBackendRequest;
use amalthea::comm::help_comm::HelpTopicSuggestion;
use amalthea::comm::help_comm::SearchHelpParams;
use amalthea::comm::help_comm::ShowHelpTopicParams;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::socket::comm::CommOutgoingTx;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::comm_msg::CommWireMsg;
use amalthea::wire::comm_open::CommOpen;
use amalthea::wire::jupyter_message::Message;
use ark::comm_handler::CommHandler;
use ark::comm_handler::CommHandlerContext;
use ark::help::r_help::RHelp;
use ark::help_proxy;
use ark::modules::ARK_ENVS;
use ark::r_task::r_task;
use ark_test::dummy_frontend::IopubExpectation;
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

    fn request(&self, request: HelpBackendRequest, id: &str) -> HelpBackendReply {
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

            let mut handler = RHelp::default();
            handler.handle_msg(msg, &ctx);
        });

        let response = self.iopub_rx.recv_comm_msg();
        let CommMsg::Rpc { id, data, .. } = response else {
            panic!("Unexpected response from help comm: {response:?}");
        };
        assert_eq!(id, request_id);
        serde_json::from_value(data).unwrap()
    }

    fn test_topic(&self, topic: &str, id: &str) {
        let request = HelpBackendRequest::ShowHelpTopic(ShowHelpTopicParams {
            topic: String::from(topic),
        });
        assert_eq!(
            self.request(request, id),
            HelpBackendReply::ShowHelpTopicReply(true)
        );
    }

    fn test_search(&self, query: &str, id: &str) {
        let request = HelpBackendRequest::SearchHelp(SearchHelpParams {
            query: String::from(query),
            search_id: String::from(id),
        });
        assert_eq!(
            self.request(request, id),
            HelpBackendReply::SearchHelpReply(true)
        );
    }

    fn get_topics(&self, query: &str, limit: i64, id: &str) -> Vec<HelpTopicSuggestion> {
        match self.request(
            HelpBackendRequest::GetHelpTopics(GetHelpTopicsParams {
                query: String::from(query),
                limit,
            }),
            id,
        ) {
            HelpBackendReply::GetHelpTopicsReply(topics) => topics,
            reply => panic!("Unexpected help reply: {reply:?}"),
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
fn test_help_search_comm() {
    let r_help = TestRHelp::new();

    r_help.test_search("linear model", "help-search-test-id");
    let topics = r_help.get_topics("plot", 50, "help-topics-test-id");
    assert!(topics.len() <= 50);
    assert_eq!(topics[0].label, "plot");
    assert!(r_help.get_topics("", 50, "empty-help-topics").is_empty());
    assert_eq!(r_help.get_topics("plot", 1, "limited-help-topics").len(), 1);
    assert!(topics
        .iter()
        .any(|topic| { topic.label == "plot" && topic.topic == "graphics::plot" }));
}

#[test]
fn test_help_search_query_edge_cases() {
    let r_help = TestRHelp::new();
    for query in [
        "[.data.frame",
        "c(",
        "^lm$",
        "regresion",
        "zzzz_ark_no_help_match",
    ] {
        r_help.test_search(query, "help-search-edge-case");
    }
    let topics = r_help.get_topics("[.data.frame", 50, "help-literal-topics");
    assert_eq!(topics[0].label, "[.data.frame");
    assert_eq!(topics[0].topic, "base::[.data.frame");

    assert!(r_task(|| {
        harp::parse_eval0(
            r#"
local({
    # Invalid regexps fall back to literal matching, with no warning escaping.
    old <- options(warn = 2)
    on.exit(options(old))
    for (query in c("[.data.frame", "c(", "\\", "[.^$|?*+(){}\\")) {
        results <- .ps.help.searchResults(query)
        stopifnot(inherits(results, "hsearch"))
        stopifnot(grepl(results$pattern, query))
        stopifnot(identical(results$type, "regexp"))
    }
    results <- .ps.help.searchResults("[.data.frame")
    stopifnot(any(results$matches[, "Entry"] == "[.data.frame"))
    stopifnot(identical(
        results,
        utils::help.search("\\[\\.data\\.frame", package = NULL)
    ))
    stopifnot(identical(
        .ps.help.searchResults("c("),
        utils::help.search("c\\(", package = NULL)
    ))

    # Valid regexps, native fuzzy matching, and no-match results are unchanged.
    for (query in c("^lm$", "regresion", "zzzz_ark_no_help_match")) {
        stopifnot(identical(
            .ps.help.searchResults(query),
            utils::help.search(query, package = NULL)
        ))
    }
    stopifnot(identical(.ps.help.searchResults("regresion")$type, "fuzzy"))
    stopifnot(nrow(.ps.help.searchResults("^lm$")$matches) > 0L)
    stopifnot(nrow(.ps.help.searchResults("zzzz_ark_no_help_match")$matches) == 0L)

    # Unrelated native search errors must still reach the caller.
    old_types <- options(help.search.types = "invalid-type")
    on.exit(options(old_types), add = TRUE)
    stopifnot(inherits(try(.ps.help.searchResults("[.data.frame"), silent = TRUE), "try-error"))
    TRUE
})
"#,
            ARK_ENVS.positron_ns,
        )
        .unwrap()
        .to::<bool>()
        .unwrap()
    }));
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
    assert!(msg.data["params"]["search_id"].is_null());
    let content = msg.data["params"]["content"].as_str().unwrap();
    assert!(content.starts_with("http://127.0.0.1:"));
    assert!(content.contains("plot"));

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Reopening the help comm (as happens on a frontend reload) must not leak the
/// previous proxy server.
///
/// The proxy's lifetime is tied to the help comm via a drop guard held in
/// `RHelp`. When the frontend opens a second `positron.help` comm, the kernel
/// replaces the handler, the old `RHelp` drops, and its proxy stops. We observe
/// this directly. Each proxy binds its own localhost port, so we connect to the
/// old port and check it stops accepting connections. Before this was wired up,
/// the old proxy stayed bound forever.
#[test]
fn test_help_proxy_torn_down_on_reopen() {
    let frontend = DummyArkFrontend::lock();

    let comm_id_1 = open_help_comm(&frontend);
    let port_1 = show_help_and_get_proxy_port(&frontend, &comm_id_1);
    assert!(proxy_is_listening(port_1));

    // Reopen the help comm, which replaces the handler and drops the old one.
    let comm_id_2 = open_help_comm(&frontend);
    let port_2 = show_help_and_get_proxy_port(&frontend, &comm_id_2);

    // The new proxy binds before the old `RHelp` is dropped, so the OS hands out
    // a different port and we can tell the two apart.
    assert_ne!(port_1, port_2);
    assert!(proxy_is_listening(port_2));

    // Teardown is asynchronous (a task inside the proxy's runtime calls
    // `stop()`), so poll until the old port refuses connections.
    wait_until_proxy_stops(port_1);
}

fn open_help_comm(frontend: &DummyArkFrontend) -> String {
    let comm_id = uuid::Uuid::new_v4().to_string();
    frontend.send_shell(CommOpen {
        comm_id: comm_id.clone(),
        target_name: String::from("positron.help"),
        data: serde_json::json!({}),
    });
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
    comm_id
}

/// Request a help topic and return the proxy port from the `show_help` URL.
fn show_help_and_get_proxy_port(frontend: &DummyArkFrontend, comm_id: &str) -> u16 {
    frontend.send_execute_request("?plot", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let msg = frontend.recv_iopub_comm_msg();
    assert_eq!(msg.comm_id, comm_id);
    assert_eq!(
        msg.data.get("method").and_then(|v| v.as_str()),
        Some("show_help")
    );
    let content = msg.data["params"]["content"].as_str().unwrap();
    let port = proxy_port_from_url(content);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    port
}

/// Pull the port out of a proxy URL like `http://127.0.0.1:<port>/...`.
fn proxy_port_from_url(url: &str) -> u16 {
    let after_host = url.strip_prefix("http://127.0.0.1:").unwrap();
    let end = after_host.find('/').unwrap();
    after_host[..end].parse().unwrap()
}

fn proxy_is_listening(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

fn wait_until_proxy_stops(port: u16) {
    for _ in 0..100 {
        if !proxy_is_listening(port) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("Proxy on port {port} is still accepting connections after teardown");
}

#[test]
fn test_help_search_navigation_correlation() {
    let frontend = DummyArkFrontend::lock();
    // This full kernel test needs HTML navigation; unit-test mode suppresses it.
    frontend.execute_request_invisibly("options(ark.testing = FALSE)");
    let comm_id = open_help_comm(&frontend);
    frontend.send_shell(CommWireMsg {
        comm_id: comm_id.clone(),
        data: serde_json::json!({
            "jsonrpc": "2.0",
            "id": "help-search-request",
            "method": "search_help",
            "params": { "query": "[.data.frame", "search_id": "ui-search" }
        }),
    });
    frontend.recv_iopub_busy();
    // Shell idle and comm delivery come from different threads and may interleave.
    let messages = frontend.recv_iopub_interleaved(&[&[IopubExpectation::Idle], &[
        IopubExpectation::CommMsg,
        IopubExpectation::CommMsg,
    ]]);
    let comms: Vec<_> = messages
        .into_iter()
        .filter_map(|message| match message {
            Message::CommMsg(message) => Some(message.content),
            _ => None,
        })
        .collect();
    assert_eq!(comms[0].comm_id, comm_id);
    assert_eq!(comms[0].data["method"], "show_help");
    assert_eq!(comms[0].data["params"]["search_id"], "ui-search");
    assert_eq!(comms[1].data["result"], true);

    // The scope has ended: console search must remain ordinary native Help.
    frontend.send_execute_request("??plot", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let event = frontend.recv_iopub_comm_msg();
    assert_eq!(event.data["method"], "show_help");
    assert!(event.data["params"]["search_id"].is_null());
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_help_index_freshness() {
    assert!(r_task(|| {
        harp::parse_eval0(r#"
local({
    old_paths <- .libPaths()
    library <- tempfile("ark-help-index-")
    dir.create(library)
    on.exit({ .libPaths(old_paths); unlink(library, recursive = TRUE) })
    dir.create(file.path(library, "stats"))
    stopifnot(file.copy(file.path(find.package("stats"), "Meta"), file.path(library, "stats"), recursive = TRUE))
    .libPaths(c(library, old_paths))
    stopifnot(length(.ps.help.getHelpTopics("", 50L)) == 0L)
    stopifnot(length(.ps.help.getHelpTopics("lm", 1L)) == 1L)
    stopifnot(identical(.ps.help.getHelpTopics(" LM ", 5L), .ps.help.getHelpTopics("lm", 5L)))
    stopifnot(inherits(try(.ps.help.getHelpTopics("lm", 0L), silent = TRUE), "try-error"))
    marker <- "zzzz_ark_help_cache_alias"
    stopifnot(length(.ps.help.getHelpTopics(marker, 50L)) == 0L)
    metadata <- file.path(library, "stats", "Meta", "hsearch.rds")
    db <- readRDS(metadata)
    alias <- db[[2L]][1L, , drop = FALSE]
    alias[1L, "Alias"] <- marker
    db[[2L]] <- rbind(db[[2L]], alias)
    saveRDS(db, metadata)
    # Force a distinguishable metadata time even on low-resolution filesystems.
    # The library directory itself has not changed.
    Sys.setFileTime(metadata, Sys.time() + 2)
    stopifnot(identical(.ps.help.getHelpTopics(marker, 50L), paste("stats", marker, sep = "\u001f")))
    stopifnot(nrow(utils::help.search(marker, agrep = FALSE)$matches) > 0L)
    unlink(file.path(library, "stats"), recursive = TRUE)
    stopifnot(length(.ps.help.getHelpTopics(marker, 50L)) == 0L)
    TRUE
})

"#, ARK_ENVS.positron_ns).unwrap().to::<bool>().unwrap()
    }));
}
