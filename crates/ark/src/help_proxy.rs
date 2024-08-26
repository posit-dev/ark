//
// help_proxy.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::net::TcpListener;

use actix_web::get;
use actix_web::http::header::ContentType;
use actix_web::web;
use actix_web::App;
use actix_web::HttpRequest;
use actix_web::HttpResponse;
use actix_web::HttpServer;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use mime_guess::from_path;
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

// Starts the help proxy.
pub fn start(target_port: u16) -> anyhow::Result<u16> {
    let source_port = HelpProxy::get_os_assigned_port()?;

    spawn!("ark-help-proxy", move || {
        match task(source_port, target_port) {
            Ok(value) => log::info!("Help proxy server exited with value: {:?}", value),
            Err(error) => log::error!("Help proxy server exited unexpectedly: {}", error),
        }
    });

    Ok(source_port)
}

// The help proxy main entry point.
#[tokio::main]
async fn task(source_port: u16, target_port: u16) -> anyhow::Result<()> {
    // Create the help proxy.
    let help_proxy = HelpProxy::new(source_port, target_port)?;

    // Run the help proxy.
    Ok(help_proxy.run().await?)
}

// AppState struct.
#[derive(Clone)]
struct AppState {
    target_port: u16,
}

// HelpProxy struct.
struct HelpProxy {
    source_port: u16,
    target_port: u16,
}

// HelpProxy implementation.
impl HelpProxy {
    // Creates a new HelpProxy.
    fn new(source_port: u16, target_port: u16) -> anyhow::Result<Self> {
        Ok(HelpProxy {
            source_port,
            target_port,
        })
    }

    // Runs the HelpProxy.
    async fn run(&self) -> anyhow::Result<()> {
        // Create the app state.
        let app_state = web::Data::new(AppState {
            target_port: self.target_port,
        });

        // Create the server.
        let server = HttpServer::new(move || {
            App::new()
                .app_data(app_state.clone())
                .service(preview_rd)
                .service(preview_img)
                .default_service(web::to(proxy_request))
        })
        .bind(("127.0.0.1", self.source_port))?;

        // Run the server.
        Ok(server.run().await?)
    }

    fn get_os_assigned_port() -> std::io::Result<u16> {
        Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
    }
}

// Proxies a request.
async fn proxy_request(req: HttpRequest, app_state: web::Data<AppState>) -> HttpResponse {
    // Get the URL path and query.
    let path = req.path();
    let query = req.query_string();

    // Construct the target URL string.
    let target_url_string = format!("http://localhost:{}{}", app_state.target_port, path);

    // Parse the target URL string into the target URL.
    let mut target_url = match Url::parse(&target_url_string) {
        Ok(url) => url,
        Err(error) => {
            log::error!("Error proxying {}: {}", target_url_string, error);
            return HttpResponse::BadGateway().finish();
        },
    };

    // Add query from original request back to URL.
    target_url.set_query(Some(query));

    // Get the target URL.
    match reqwest::get(target_url.clone()).await {
        // OK.
        Ok(response) => {
            // We only handle OK. Everything else is unexpected.
            if response.status() != reqwest::StatusCode::OK {
                return HttpResponse::BadGateway().finish();
            }

            // Get the headers we need.
            let headers = response.headers().clone();
            let content_type = headers.get("content-type");

            // Log.
            log::info!(
                "Proxying URL '{:?}' path '{}' content-type is '{:?}'",
                target_url.to_string(),
                target_url.path(),
                content_type,
            );

            // Build and return the response.
            let mut http_response_builder = HttpResponse::Ok();
            if content_type.is_some() {
                http_response_builder.content_type(content_type.unwrap());
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

    let content = r_task(|| unsafe {
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
