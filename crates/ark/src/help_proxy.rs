//
// help_proxy.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::error::Error;
use std::time::Duration;

use actix_web::get;
use actix_web::http::header::ContentType;
use actix_web::http::uri::PathAndQuery;
use actix_web::web;
use actix_web::App;
use actix_web::HttpRequest;
use actix_web::HttpResponse;
use actix_web::HttpServer;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use mime_guess::from_path;
use reqwest::Client;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::RetryTransientMiddleware;
use rust_embed::RustEmbed;
use serde::Deserialize;
use stdext::spawn;
use stdext::unwrap;
use url::Url;

use crate::r_task;

// Embed `resources/help/` which is where replacement resources can be found.
#[derive(RustEmbed)]
#[folder = "resources/help/"]
struct Asset;

#[derive(Deserialize)]
struct PreviewRdParams {
    file: String,
}

struct AppState {
    target_port: u16,
}

// Starts the help proxy.
pub fn start(target_port: u16) -> anyhow::Result<u16> {
    let (port_tx, port_rx) = crossbeam::channel::bounded::<u16>(1);

    spawn!("ark-help-proxy", move || -> anyhow::Result<()> {
        // Bind to port `0` to allow the OS to assign the port, avoiding any race conditions
        let address = "127.0.0.1:0";

        let server = HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(AppState { target_port }))
                .service(preview_rd)
                .service(preview_img)
                .default_service(web::to(proxy_request))
        })
        .bind(address)?
        .workers(1);

        // Get finalized address post `bind()`
        let addresses = server.addrs();
        let Some(address) = addresses.first() else {
            return Err(anyhow::anyhow!(
                "Help proxy server failed to finalize address"
            ));
        };

        // Send back the finalized port address
        port_tx.send(address.port())?;

        // Create a single-threaded Tokio runtime to spare stack memory. The
        // help proxy server does not need to be high performance.
        // Note that `new_current_thread()` seems to consume much more memory.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()?;

        // Execute the task within the runtime.
        rt.block_on(async {
            match server.run().await {
                Ok(value) => log::info!("Help proxy server exited with value: {:?}", value),
                Err(error) => log::error!("Help proxy server exited unexpectedly: {}", error),
            }
        });

        Ok(())
    });

    // Wait for the returned port with an extensive timeout
    match port_rx.recv_timeout(Duration::from_secs(20)) {
        Ok(port) => Ok(port),
        Err(err) => Err(anyhow::anyhow!(
            "Help proxy server timed out while waiting for a port: {err:?}"
        )),
    }
}

// Proxies a request.
async fn proxy_request(req: HttpRequest, app_state: web::Data<AppState>) -> HttpResponse {
    let target_port = app_state.target_port;

    let target_path_and_query = req
        .uri()
        .path_and_query()
        .map(PathAndQuery::as_str)
        .unwrap_or_default();

    // Construct the target URL string. Use `127.0.0.1` rather than `localhost`:
    // on Windows `localhost` resolves to both `::1` and `127.0.0.1`, but R's
    // `tools::startDynamicHelp()` binds to IPv4 only, and R itself hands us URLs
    // formed with `127.0.0.1`.
    let target_url_string = format!("http://127.0.0.1:{target_port}{target_path_and_query}");

    // Parse the target URL string into the target URL.
    let target_url = match Url::parse(&target_url_string) {
        Ok(url) => url,
        Err(error) => {
            log::error!("Error proxying {}: {}", target_url_string, error);
            return HttpResponse::BadGateway().finish();
        },
    };

    // Set up reqwest client with 3 retries (posit-dev/positron#3753).
    // Disable proxy since we're connecting to localhost; otherwise HTTP_PROXY
    // and other env vars can cause the request to get incorrectly routed to a
    // proxy.
    //
    // Note: `RetryTransientMiddleware` only retries the request *setup* (before
    // headers arrive). Body-decode failures after headers have been received
    // (seen intermittently on Windows CI against R's HTTPD) are retried
    // separately in the loop below.
    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let reqwest_client = match Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(error) => {
            log::error!("Failed to create reqwest client: {error}");
            return HttpResponse::InternalServerError().finish();
        },
    };
    let client = ClientBuilder::new(reqwest_client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();

    // Certain resources are served from our embedded bundle instead of from R.
    let replacement_embedded_file = match target_url.path().to_lowercase() {
        path if path.ends_with("r.css") => Asset::get("R.css"),
        path if path.ends_with("prism.css") => Asset::get("prism.css"),
        _ => None,
    };

    // Retry the full GET (including body read) on transient failures: both
    // body-decode errors and 200-with-empty-body responses (the latter seen on
    // Windows CI, where the Help pane renders a blank page). The R HTTPD will
    // re-serve the same page, so retrying is safe.
    const MAX_BODY_RETRIES: u32 = 3;

    // Why the most recent attempt failed; used to pick a final response once all
    // retries are exhausted.
    enum BodyFailure {
        // `response.bytes()` errored; carries the source chain.
        Decode(String),
        // 200 OK with an empty body.
        Empty,
    }
    let mut last_failure: Option<BodyFailure> = None;

    for attempt in 0..=MAX_BODY_RETRIES {
        let response = match client.get(target_url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                log::error!(
                    "Error proxying {target_url}: {chain}",
                    chain = error_source_chain(&error),
                );
                return HttpResponse::BadGateway().finish();
            },
        };

        // Snapshot headers we'll log and forward downstream.
        let headers = response.headers().clone();
        let content_type = headers.get("content-type").cloned();

        // Log content-length and transfer-encoding too: body-decode failures
        // we see on Windows CI may be due to chunked-encoding or truncation
        // issues against R's HTTPD, and these headers narrow it down.
        log::info!(
            "Proxying URL {url:?} path '{path}' content-type {ct:?} content-length {cl:?} transfer-encoding {te:?}",
            url = target_url.to_string(),
            path = target_url.path(),
            ct = content_type,
            cl = headers.get("content-length"),
            te = headers.get("transfer-encoding"),
        );

        // We only handle OK. Everything else is unexpected and not worth retrying.
        if response.status() != reqwest::StatusCode::OK {
            log::error!(
                "Got status {status} proxying {url:?}: {body}",
                status = response.status(),
                url = target_url.to_string(),
                body = match response.text().await {
                    Ok(text) => text,
                    Err(err) => format!("Response error: {err:?}"),
                },
            );
            return HttpResponse::BadGateway().finish();
        }

        let mut http_response_builder = HttpResponse::Ok();
        if let Some(content_type) = content_type.as_ref() {
            http_response_builder.content_type(convert_header_value(content_type));
        }

        // For embedded replacements we don't need R's body at all; serve the
        // bundled bytes directly. `Cow<'static, [u8]>::clone()` on a `Borrowed`
        // variant is just a pointer copy.
        if let Some(replacement) = replacement_embedded_file.as_ref() {
            return http_response_builder.body(replacement.data.clone());
        }

        match response.bytes().await {
            Ok(body) if body.is_empty() => {
                // A 200 with no body is never a valid help page. Treat it as a
                // transient failure and retry; log the framing headers so we can
                // tell an explicit `content-length: 0` from a silently truncated
                // chunked response.
                log::warn!(
                    "Empty body (attempt {n}/{total}) for {target_url_string}: content-length {cl:?} transfer-encoding {te:?}",
                    n = attempt + 1,
                    total = MAX_BODY_RETRIES + 1,
                    cl = headers.get("content-length"),
                    te = headers.get("transfer-encoding"),
                );
                last_failure = Some(BodyFailure::Empty);
            },
            Ok(body) => return http_response_builder.body(body),
            Err(error) => {
                // Walk the source chain: reqwest's top-level error often hides
                // the actual cause (truncated chunk, decompression failure,
                // socket close).
                let chain = error_source_chain(&error);
                log::warn!(
                    "Body read failed (attempt {n}/{total}) for {target_url_string}: {chain}",
                    n = attempt + 1,
                    total = MAX_BODY_RETRIES + 1,
                );
                last_failure = Some(BodyFailure::Decode(chain));
            },
        }
    }

    // Every attempt failed. An empty body is plausibly what R really served, so
    // pass the empty 200 through (matching pre-retry behavior); a decode error
    // means we never got a usable body, so report a gateway error.
    match last_failure {
        Some(BodyFailure::Empty) => {
            log::error!(
                "Empty body after {attempts} attempts for {target_url_string}; serving empty 200 response",
                attempts = MAX_BODY_RETRIES + 1,
            );
            HttpResponse::Ok().finish()
        },
        Some(BodyFailure::Decode(err)) => {
            log::error!(
                "Error proxying {target_url_string}: body read failed after {attempts} attempts: {err}",
                attempts = MAX_BODY_RETRIES + 1,
            );
            HttpResponse::BadGateway().finish()
        },
        None => {
            // Unreachable: the loop only exits here after recording a failure.
            log::error!(
                "Error proxying {target_url_string}: retries exhausted with no recorded failure"
            );
            HttpResponse::BadGateway().finish()
        },
    }
}

#[get("/preview")]
async fn preview_rd(params: web::Query<PreviewRdParams>) -> HttpResponse {
    let file = params.file.as_str();

    log::info!("Received request with path 'preview' and file '{file}'.");

    if !std::path::Path::new(file).exists() {
        log::error!("File does not exist: '{file}'.");
        return HttpResponse::BadGateway().finish();
    }

    let content = r_task(|| {
        RFunction::from(".ps.Rd2HTML")
            .param("rd_file", file)
            .call()
            .and_then(|content| content.to::<String>())
    });

    let content = unwrap!(content, Err(err) => {
        log::error!("Error converting Rd to HTML: {err:?}");
        return HttpResponse::InternalServerError().finish();
    });

    HttpResponse::Ok()
        .content_type(ContentType::html())
        .body(content)
}

#[get("/dev-figure")]
async fn preview_img(params: web::Query<PreviewRdParams>) -> HttpResponse {
    let file = params.file.as_str();

    log::info!("Received request with path 'dev-figure' for image file '{file}'.");

    if !std::path::Path::new(file).exists() {
        log::error!("File does not exist: '{file}'.");
        return HttpResponse::BadGateway().finish();
    }

    let mime_type = from_path(file).first();
    let mime_str = match mime_type {
        Some(mime) => mime.to_string(),
        None => {
            log::error!("Could not determine MIME type.");
            return HttpResponse::InternalServerError().finish();
        },
    };

    let content = match tokio::fs::read(file).await {
        Ok(content) => content,
        Err(err) => {
            log::error!("Error reading image file: {err:?}");
            return HttpResponse::InternalServerError().finish();
        },
    };

    HttpResponse::Ok().content_type(mime_str).body(content)
}

// Conversion helper between reqwest and actix-web's `HeaderValue`
//
// Both point to a re-exported `HeaderValue` from the http crate, but they come from
// different versions of the http crate, so they look different to Rust and we need a
// small conversion helper.
// - reqwest re-exports http 1.0.0's `HeaderValue`
// - actix-web re-exports http 0.2.7's `HeaderValue`
fn convert_header_value(x: &reqwest::header::HeaderValue) -> actix_web::http::header::HeaderValue {
    let out = actix_web::http::header::HeaderValue::from_bytes(x.as_bytes());

    // We've checked these are the same underlying structure, so assert that this works.
    let mut out = out.unwrap();

    // Set the one other field that defaults to `false`, just in case.
    out.set_sensitive(x.is_sensitive());

    out
}

// Walks an error's source chain into a single string. Reqwest's top-level error
// message often hides the underlying cause (e.g. truncated chunk, decompression
// failure, socket close), so we surface the whole chain in logs.
fn error_source_chain(error: &dyn Error) -> String {
    use std::fmt::Write;
    let mut chain = format!("{error}");
    let mut source = error.source();
    while let Some(err) = source {
        let _ = write!(chain, " -> {err}");
        source = err.source();
    }
    chain
}
