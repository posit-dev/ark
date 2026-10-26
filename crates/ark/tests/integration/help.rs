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
            package_name: None,
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
    assert!(topics.iter().any(|topic| {
        topic.label == "plot" &&
            topic.topic == "plot" &&
            topic.package.as_deref() == Some("graphics")
    }));
}

#[test]
fn test_help_search_empty_query() {
    let r_help = TestRHelp::new();
    for query in ["", " ", "\t\n"] {
        assert_eq!(
            r_help.request(
                HelpBackendRequest::SearchHelp(SearchHelpParams {
                    query: String::from(query),
                    search_id: String::from("empty-search"),
                }),
                "empty-search",
            ),
            HelpBackendReply::SearchHelpReply(false)
        );
    }
}

#[test]
fn test_help_search_padded_query() {
    assert!(r_task(|| {
        harp::parse_eval0(
            r#"
local({
    expected <- .ps.help.searchUrl("lm")
    stopifnot(is.character(expected))
    for (query in c(" lm ", "\tlm\n")) {
        stopifnot(identical(.ps.help.searchUrl(query), expected))
    }
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
    assert_eq!(topics[0].topic, "[.data.frame");
    assert_eq!(topics[0].package.as_deref(), Some("base"));

    assert!(r_task(|| {
        harp::parse_eval0(
            r#"
local({
    old <- options(warn = 2)
    on.exit(options(old))

    # Invalid regexps and operator queries are searched as literal text.
    for (query in c("[.data.frame", "c(", "\\", "[.^$|?*+(){}\\", "$", "?", "+", "*", "|", ".", "[[")) {
        pattern <- .ps.help.searchPattern(query)
        stopifnot(!identical(pattern, query))
        stopifnot(grepl(pattern, query))
        stopifnot(inherits(utils::help.search(pattern, package = NULL), "hsearch"))
    }
    stopifnot(identical(.ps.help.searchPattern("[.data.frame"), "\\[\\.data\\.frame"))
    stopifnot(identical(.ps.help.searchPattern("$"), "\\$"))
    results <- utils::help.search(.ps.help.searchPattern("$"), package = NULL)
    stopifnot(any(results$matches[, "Entry"] == "$"))
    stopifnot(nrow(results$matches) < nrow(utils::help.search("$", package = NULL)$matches))

    # Valid regexps keep native regexp and fuzzy matching.
    for (query in c("^lm$", "regresion", "linear model", "data.frame", "zzzz_ark_no_help_match")) {
        stopifnot(identical(.ps.help.searchPattern(query), query))
    }

    # Each query has its own URL, which the help server answers with that query's results.
    page <- function(query) {
        url <- .ps.help.searchUrl(query)
        stopifnot(startsWith(url, "http://127.0.0.1:"))
        pattern <- utils::URLdecode(sub(".*[?]pattern=", "", url))
        stopifnot(identical(pattern, .ps.help.searchPattern(query)))
        tools:::httpd("/doc/html/Search", c(pattern = pattern))$payload
    }
    stopifnot(!identical(.ps.help.searchUrl("glm"), .ps.help.searchUrl("^lm$")))
    stopifnot(grepl("library/stats/help/glm\"", page("glm"), fixed = TRUE))
    stopifnot(!grepl("library/stats/help/glm\"", page("^lm$"), fixed = TRUE))
    stopifnot(grepl(">base::[.data.frame</a>", page("[.data.frame"), fixed = TRUE))

    # The results page shows the pattern unescaped, so it must not contain markup.
    for (query in c("<script>alert(1)</script>", "<img src=x onerror=alert(1)>", "<!--", "<?x")) {
        shown <- sub('(?s).*The search string was <b>"(.*?)"</b>.*', "\\1", page(query), perl = TRUE)
        stopifnot(!grepl("<[[:alpha:]/!?]", shown))
    }
    # The escape keeps the regexp meaning.
    stopifnot(identical(.ps.help.searchPattern("<script>"), "\\x3Cscript>"))
    stopifnot(grepl(.ps.help.searchPattern("<script>"), "<script>"))
    for (query in c("<-", "<<-", "<=", "%<>%")) {
        stopifnot(identical(.ps.help.searchPattern(query), query))
    }
    # The start-of-word anchor keeps its meaning as a word boundary.
    stopifnot(identical(.ps.help.searchPattern("\\<lm"), "\\blm"))
    stopifnot(identical(
        utils::help.search(.ps.help.searchPattern("\\<lm"), package = NULL)$matches,
        utils::help.search("\\<lm", package = NULL)$matches
    ))
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
    r_task(|| {
        harp::parse_eval_global("called <- FALSE").unwrap();
    });
    r_help.test_topic(
        "base::list(hello = obj$hello)$hello",
        "qualified-expression",
    );
    assert!(r_task(|| harp::parse_eval_global("called").unwrap().to::<bool>()).unwrap());
}

#[test]
fn test_qualified_topic_preserves_custom_handler() {
    let r_help = TestRHelp::new();
    r_task(|| {
        harp::parse_eval_global(
            r#"
            # Give a documented export an explicit class, as reticulate objects have.
            ns <- asNamespace("stats")
            unlockBinding("median", ns)
            assign("median", structure(stats::median, class = "help_test"), ns)
            lockBinding("median", ns)
            called <- FALSE
            .ark.register_method("ark_positron_help_get_handler", "help_test", function(x) {
                function() { called <<- TRUE }
            })
            stopifnot(length(utils::help("median", package = "stats")) > 0L)
        "#,
        )
        .unwrap();
    });
    r_help.test_topic("stats::median", "custom-handler-first");
    assert!(r_task(|| harp::parse_eval_global("called").unwrap().to::<bool>()).unwrap());
    r_task(|| {
        harp::parse_eval_global("called <- FALSE").unwrap();
    });
    assert_eq!(
        r_help.request(
            HelpBackendRequest::ShowHelpTopic(ShowHelpTopicParams {
                topic: String::from("median"),
                package_name: Some(String::from("stats")),
            }),
            "literal-suggestion"
        ),
        HelpBackendReply::ShowHelpTopicReply(true)
    );
    assert!(!r_task(|| harp::parse_eval_global("called").unwrap().to::<bool>()).unwrap());
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
    for help_type in ["html", "text"] {
        frontend.execute_request_invisibly(&format!("options(help_type = '{help_type}')"));
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
        let url = comms[0].data["params"]["content"].as_str().unwrap();
        assert!(url.contains("/doc/html/Search?pattern=%5C%5B%5C.data%5C.frame"));
        assert_eq!(comms[1].comm_id, comm_id);
        assert_eq!(comms[1].data["result"], true);
        frontend.execute_request_invisibly(&format!(
            "stopifnot(identical(getOption('help_type'), '{help_type}'))"
        ));
    }
    frontend.execute_request_invisibly("options(help_type = 'html')");

    // The scope has ended: console search must remain ordinary native Help.
    frontend.send_execute_request("??plot", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let event = frontend.recv_iopub_comm_msg();
    assert_eq!(event.data["method"], "show_help");
    assert!(event.data["params"]["search_id"].is_null());
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("options(ark.testing = TRUE)");
}

#[test]
fn test_help_index_freshness() {
    assert!(r_task(|| {
        harp::parse_eval0(r#"
local({
    # Control elapsed time and count package scans without sleeping or needing NFS.
    cache <- environment(.ps.help.searchIndex)
    now <- 100
    scans <- 0L
    metadata_checks <- 0L
    builds <- 0L
    cache$unique <- function(...) {
        builds <<- builds + 1L
        base::unique(...)
    }
    cache$proc.time <- function() c(elapsed = now)
    cache$list.dirs <- function(...) {
        scans <<- scans + 1L
        base::list.dirs(...)
    }
    cache$file.info <- function(paths, ...) {
        if (any(endsWith(paths, ".rds"))) {
            metadata_checks <<- metadata_checks + 1L
        }
        base::file.info(paths, ...)
    }
    on.exit(rm(list = c("proc.time", "list.dirs", "file.info", "unique"), envir = cache), add = TRUE)
    stopifnot(.ps.help.searchHelp("lm"))
    stopifnot(is.null(cache$index), builds == 0L)
    expected <- .ps.help.getHelpTopics("lm", 5L)
    stopifnot(builds == 1L)
    stopifnot(.ps.help.searchHelp("lm"))
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), expected))
    stopifnot(builds == 1L)
    old_paths <- .libPaths()
    library <- tempfile("ark-help-index-")
    dir.create(library)
    on.exit({ .libPaths(old_paths); unlink(library, recursive = TRUE) }, add = TRUE)
    dir.create(file.path(library, "stats"))
    stopifnot(file.copy(file.path(find.package("stats"), "Meta"), file.path(library, "stats"), recursive = TRUE))
    before_paths_change <- scans
    .libPaths(c(library, old_paths))
    stopifnot(nrow(.ps.help.getHelpTopics("", 50L)) == 0L)
    stopifnot(nrow(.ps.help.getHelpTopics("lm", 1L)) == 1L)
    stopifnot(identical(.ps.help.getHelpTopics(" LM ", 5L), .ps.help.getHelpTopics("lm", 5L)))
    stopifnot(scans == before_paths_change + length(.libPaths()))
    stopifnot(builds == 2L)
    initial_scans <- scans
    initial_checks <- metadata_checks
    marker <- "zzzz_ark_help_cache_alias"
    stopifnot(nrow(.ps.help.getHelpTopics(marker, 50L)) == 0L)
    metadata <- file.path(library, "stats", "Meta", "hsearch.rds")
    db <- readRDS(metadata)
    alias <- db[[2L]][1L, , drop = FALSE]
    alias[1L, "Alias"] <- marker
    db[[2L]] <- rbind(db[[2L]], alias)
    saveRDS(db, metadata)
    # Force a distinguishable metadata time even on low-resolution filesystems.
    # The library directory itself has not changed.
    Sys.setFileTime(metadata, Sys.time() + 2)
    now <- 159
    stopifnot(nrow(.ps.help.getHelpTopics(marker, 50L)) == 0L)
    stopifnot(scans == initial_scans, metadata_checks == initial_checks)
    now <- 160
    stopifnot(.ps.help.searchHelp(marker))
    stopifnot(is.null(cache$index), builds == 2L)
    stopifnot(identical(unname(as.list(.ps.help.getHelpTopics(marker, 50L))), list("stats", marker)))
    stopifnot(scans == initial_scans + length(.libPaths()))
    stopifnot(metadata_checks == initial_checks + 1L, builds == 3L)
    # Repeated requests do not repeat the full scan after expiry has been handled.
    .ps.help.getHelpTopics(marker, 50L)
    stopifnot(metadata_checks == initial_checks + 1L, builds == 3L)
    stopifnot(nrow(utils::help.search(marker, agrep = FALSE)$matches) > 0L)
    unlink(file.path(library, "stats"), recursive = TRUE)
    Sys.setFileTime(library, Sys.time() + 4)
    stopifnot(nrow(.ps.help.getHelpTopics(marker, 50L)) == 0L)
    stopifnot(metadata_checks == initial_checks + 2L)
    TRUE
})

"#, ARK_ENVS.positron_ns).unwrap().to::<bool>().unwrap()
    }));
}

#[test]
fn test_help_index_failed_rebuild() {
    assert!(r_task(|| {
        harp::parse_eval0(
            r#"
local({
    old_paths <- .libPaths()
    library <- tempfile("ark-help-broken-")
    dir.create(library)
    on.exit({ .libPaths(old_paths); unlink(library, recursive = TRUE) }, add = TRUE)
    package <- file.path(library, "stats")
    dir.create(package)
    stopifnot(all(file.copy(
        file.path(find.package("stats"), c("Meta", "DESCRIPTION")),
        package,
        recursive = TRUE
    )))
    metadata <- file.path(package, "Meta", "hsearch.rds")
    healthy <- readRDS(metadata)
    writeLines("not an R serialization", metadata)
    .libPaths(c(library, old_paths))

    cache <- environment(.ps.help.searchIndex)
    now <- 100
    cache$proc.time <- function() c(elapsed = now)
    on.exit(rm("proc.time", envir = cache), add = TRUE)
    rebuilds <- 0L
    suppressMessages(trace(
        "help.search",
        where = asNamespace("utils"),
        tracer = function() { rebuilds <<- rebuilds + 1L },
        print = FALSE
    ))
    on.exit(suppressMessages(untrace("help.search", where = asNamespace("utils"))), add = TRUE)
    empty <- data.frame(package = character(), label = character())

    # A real corrupt file fails the initial rebuild, with no usable index.
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(rebuilds == 1L, is.null(cache$matches), is.null(cache$index))
    stopifnot(cache$last_checked == now)
    now <- 159
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(isFALSE(.ps.help.searchIndex(build = FALSE)), is.null(.ps.help.searchUrl("lm")))
    stopifnot(rebuilds == 1L)

    # Unchanged metadata must still permit a retry when the cooldown expires.
    now <- 160
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(rebuilds == 2L)
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(rebuilds == 2L)

    # A library-path change permits an immediate retry during the cooldown.
    extra <- tempfile("ark-help-extra-")
    dir.create(extra)
    on.exit(unlink(extra, recursive = TRUE), add = TRUE)
    .libPaths(c(extra, library, old_paths))
    now <- 161
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(rebuilds == 3L)

    # Repair in place: detection waits until the next metadata check.
    saveRDS(healthy, metadata)
    Sys.setFileTime(metadata, Sys.time() + 2)
    now <- 220
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(rebuilds == 3L)
    now <- 221
    recovered <- .ps.help.getHelpTopics("lm", 5L)
    stopifnot(nrow(recovered) > 0L, recovered$label[[1L]] == "lm")
    stopifnot(rebuilds == 4L)
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), recovered))
    stopifnot(rebuilds == 4L)

    # A later failure discards the previously successful index.
    writeLines("not an R serialization", metadata)
    Sys.setFileTime(library, Sys.time() + 4)
    now <- 222
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(rebuilds == 5L, is.null(cache$index), is.null(cache$matches))
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), empty))
    stopifnot(rebuilds == 5L)

    # A changed library signature also allows recovery before the cooldown ends.
    saveRDS(healthy, metadata)
    Sys.setFileTime(library, Sys.time() + 6)
    now <- 223
    stopifnot(identical(.ps.help.getHelpTopics("lm", 5L), recovered))
    stopifnot(rebuilds == 6L)
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
fn test_help_punctuation_suggestions_open_exact_target() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly("options(ark.testing = FALSE)");
    let comm_id = open_help_comm(&frontend);
    // R versions differ in whether `$.data.frame` is documented. Add a fixture
    // alias to a copied help database so its suggestion always round-trips.
    frontend.execute_request_invisibly(
        r#"
        alias_library <- tempfile("ark-help-aliases-")
        dir.create(alias_library)
        alias_package <- file.path(alias_library, "arkhelpaliases")
        dir.create(alias_package)
        writeLines(c("Package: arkhelpaliases", "Version: 1.0"),
            file.path(alias_package, "DESCRIPTION"))
        stopifnot(all(file.copy(file.path(find.package("base"), c("Meta", "help")),
            alias_package, recursive = TRUE)))
        for (extension in c("rdb", "rdx")) {
            stopifnot(file.rename(
                file.path(alias_package, "help", paste0("base.", extension)),
                file.path(alias_package, "help", paste0("arkhelpaliases.", extension))))
        }
        metadata <- file.path(alias_package, "Meta", "package.rds")
        description <- readRDS(metadata)
        description$DESCRIPTION["Package"] <- "arkhelpaliases"
        saveRDS(description, metadata)
        metadata <- file.path(alias_package, "Meta", "hsearch.rds")
        db <- readRDS(metadata)
        for (i in seq_along(db)) db[[i]][, "Package"] <- "arkhelpaliases"
        db[[2L]][db[[2L]][, "Alias"] == "$", "Alias"] <- "$.data.frame"
        saveRDS(db, metadata)
        metadata <- file.path(alias_package, "help", "aliases.rds")
        aliases <- readRDS(metadata)
        aliases["$.data.frame"] <- "Extract"
        saveRDS(aliases, metadata)
        old_alias_paths <- .libPaths()
        .libPaths(c(alias_library, old_alias_paths))
    "#,
    );

    let mut has_data_table = false;
    frontend.execute_request(
        "isTRUE(requireNamespace('data.table', quietly = TRUE))",
        |result| has_data_table = result.contains("TRUE"),
    );

    for (package, alias, target) in [
        ("arkhelpaliases", "$.data.frame", "Extract"),
        ("data.table", ":=", "assign"),
        ("base", "$", "Extract"),
        ("base", "$<-", "Extract"),
        ("base", "@", "slotOp"),
        ("base", "@<-", "slotOp"),
        ("base", ":", "Colon"),
        ("base", "::", "ns-dblcolon"),
        ("base", ":::", "ns-dblcolon"),
        ("methods", "$<-,envRefClass-method", "stdRefClass"),
        ("methods", "$<-,localRefClass-method", "localRefClass"),
    ] {
        if package == "data.table" && !has_data_table {
            continue;
        }
        // Select the exact suggestion and round-trip it through the Help RPC.
        frontend.send_shell(CommWireMsg {
            comm_id: comm_id.clone(),
            data: serde_json::json!({
                "jsonrpc": "2.0", "id": "aliases", "method": "get_help_topics",
                "params": { "query": alias, "limit": 50 }
            }),
        });
        frontend.recv_iopub_busy();
        let messages = frontend
            .recv_iopub_interleaved(&[&[IopubExpectation::Idle], &[IopubExpectation::CommMsg]]);
        let reply = messages
            .into_iter()
            .find_map(|message| match message {
                Message::CommMsg(message) => Some(message.content.data),
                _ => None,
            })
            .unwrap();
        let suggestions: Vec<HelpTopicSuggestion> =
            serde_json::from_value(reply["result"].clone()).unwrap();
        let suggestion = suggestions
            .iter()
            .find(|suggestion| {
                suggestion.label == alias && suggestion.detail.as_deref() == Some(package)
            })
            .unwrap();
        assert_eq!(suggestion.topic, alias);
        assert_eq!(suggestion.package.as_deref(), Some(package));
        assert_help_topic_target(
            &frontend,
            &comm_id,
            &suggestion.topic,
            suggestion.package.as_deref(),
            package,
            target,
        );
    }
    for (topic, target) in [
        (":", "Colon"),
        ("::", "ns-dblcolon"),
        (":::", "ns-dblcolon"),
    ] {
        assert_help_topic_target(&frontend, &comm_id, topic, None, "base", target);
    }
    frontend.execute_request_invisibly(
        ".libPaths(old_alias_paths); unlink(alias_library, recursive = TRUE)",
    );
    frontend.execute_request_invisibly("options(ark.testing = TRUE)");
}

fn assert_help_topic_target(
    frontend: &DummyArkFrontend,
    comm_id: &str,
    topic: &str,
    request_package: Option<&str>,
    package: &str,
    target: &str,
) {
    frontend.send_shell(CommWireMsg {
        comm_id: String::from(comm_id),
        data: serde_json::json!({
            "jsonrpc": "2.0", "id": "open-alias", "method": "show_help_topic",
            "params": { "topic": topic, "package_name": request_package }
        }),
    });
    frontend.recv_iopub_busy();
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
    assert_eq!(comms[0].data["method"], "show_help");
    let url = comms[0].data["params"]["content"].as_str().unwrap();
    assert!(url.ends_with(&format!("/library/{package}/html/{target}.html")));
    assert_eq!(comms[1].data["result"], true);
}

#[test]
fn test_help_qualified_topic_splitting() {
    assert!(r_task(|| {
        harp::parse_eval0(
            r#"
local({
    for (alias in c("$", "$<-", "$.data.frame", "@", "@<-", ":", "::", ":::")) {
        stopifnot(identical(split_topic(paste0("base::", alias)),
            list(topic = alias, package = "base")))
    }
    for (alias in c(":", "::", ":::")) {
        stopifnot(identical(split_topic(alias), list(topic = alias, package = NULL)))
    }
    stopifnot(identical(split_topic("utils:::find"),
        list(topic = "find", package = "utils")))
    stopifnot(identical(split_topic("tensorflow::tf$abs"),
        list(topic = "tf$abs", package = "tensorflow")))
    stopifnot(identical(split_topic("obj$hello"),
        list(topic = "obj$hello", package = NULL)))
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
fn test_help_topic_missing_package() {
    // A suggested package can be removed before the suggestion is opened.
    assert!(r_task(|| {
        harp::parse_eval0(
            r#"
local({
    stopifnot(isFALSE(.ps.help.showHelpTopic("abs", package = "zzzzarkmissingpackage")))
    stopifnot(isFALSE(.ps.help.showHelpTopic("zzzzarkmissingpackage::abs")))
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
