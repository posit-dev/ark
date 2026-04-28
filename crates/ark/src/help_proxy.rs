//
// help_proxy.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::time::Duration;

use actix_web::get;
use actix_web::http::header::ContentType;
use actix_web::web;
use actix_web::App;
use actix_web::HttpRequest;
use actix_web::HttpResponse;
use actix_web::HttpServer;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use http::uri::PathAndQuery;
use mime_guess::from_path;
use reqwest::Client;
use reqwest_middleware::ClientBuilder;
use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::RetryTransientMiddleware;
use rust_embed::RustEmbed;
use serde::Deserialize;
use stdext::result::ResultExt;
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

    // Construct the target URL string.
    let target_url_string = format!("http://localhost:{target_port}{target_path_and_query}");

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

    // Get the target URL.
    match client.get(target_url.clone()).send().await {
        // OK.
        Ok(response) => {
            // Get the headers we need.
            let headers = response.headers().clone();
            let content_type = headers.get("content-type");

            // Log.
            log::info!(
                "Proxying URL {:?} path '{}' content-type is '{:?}'",
                target_url.to_string(),
                target_url.path(),
                content_type,
            );

            // We only handle OK. Everything else is unexpected.
            if response.status() != reqwest::StatusCode::OK {
                log::error!(
                    "Got status {status} proxying {url:?}: {response:?}",
                    status = response.status(),
                    url = target_url.to_string(),
                    response = match response.text().await {
                        Ok(response) => response,
                        Err(err) => format!("Response error: {err:?}"),
                    },
                );
                return HttpResponse::BadGateway().finish();
            }

            // Build and return the response.
            let mut http_response_builder = HttpResponse::Ok();
            if let Some(content_type) = content_type {
                let content_type = convert_header_value(content_type);
                http_response_builder.content_type(content_type);
            }

            // Certain resources are replaced.
            let replacement_embedded_file = match target_url.path().to_lowercase() {
                path if path.ends_with("r.css") => Asset::get("R.css"),
                path if path.ends_with("prism.css") => Asset::get("prism.css"),
                _ => None,
            };

            // Return the replacement resource or the real resource.
            match replacement_embedded_file {
                Some(replacement_embedded_file) => {
                    http_response_builder.body(replacement_embedded_file.data)
                },
                None => http_response_builder.body(match response.bytes().await {
                    Ok(body) => body,
                    Err(error) => {
                        log::error!("Error proxying {}: {}", target_url_string, error);
                        return HttpResponse::BadGateway().finish();
                    },
                }),
            }
        },
        // Error.
        Err(error) => {
            log::error!("Error proxying {}: {}", target_url, error);
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
