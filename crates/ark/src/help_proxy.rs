//
// help_proxy.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::net::TcpListener;

use actix_web::get;
use actix_web::web;
use actix_web::App;
use actix_web::HttpResponse;
use actix_web::HttpServer;
use stdext::spawn;
use url::Url;

use crate::browser;

// Starts the help proxy.
pub fn start(target_port: u16) {
    spawn!("ark-help-proxy", move || {
        match task(target_port) {
            Ok(value) => log::info!("Help proxy server exited with value: {:?}", value),
            Err(error) => log::error!("Help proxy server exited unexpectedly: {}", error),
        }
    });
}

// The help proxy main entry point.
#[tokio::main]
async fn task(target_port: u16) -> anyhow::Result<()> {
    // Create the help proxy.
    let help_proxy = HelpProxy::new(target_port)?;

    // Set the help proxy port.
    unsafe { browser::PORT = help_proxy.source_port };

    // Run the help proxy.
    Ok(help_proxy.run().await?)
}

// AppState struct.
#[derive(Clone)]
struct AppState {
    pub target_port: u16,
}

// HelpProxy struct.
struct HelpProxy {
    pub source_port: u16,
    pub target_port: u16,
}

// HelpProxy implementation.
impl HelpProxy {
    // Creates a new HelpProxy.
    pub fn new(target_port: u16) -> anyhow::Result<Self> {
        Ok(HelpProxy {
            source_port: TcpListener::bind("127.0.0.1:0")?.local_addr()?.port(),
            target_port,
        })
    }

    // Runs the HelpProxy.
    pub async fn run(&self) -> anyhow::Result<()> {
        // Create the app state.
        let app_state = web::Data::new(AppState {
            target_port: self.target_port,
        });

        // Create the server.
        let server = HttpServer::new(move || {
            App::new()
                .app_data(app_state.clone())
                .service(proxy_request)
        })
        .bind(("127.0.0.1", self.source_port))?;

        // Run the server.
        Ok(server.run().await?)
    }
}

// Proxies a request.
#[get("/{url:.*}")]
async fn proxy_request(path: web::Path<(String,)>, app_state: web::Data<AppState>) -> HttpResponse {
    // Get the URL path.
    let (path,) = path.into_inner();

    // Construct the target URL string.
    let target_url_string = format!("http://localhost:{}/{path}", app_state.target_port);

    // Parse the target URL string into the target URL.
    let target_url = match Url::parse(&target_url_string) {
        Ok(url) => url,
        Err(error) => {
            log::info!("Error proxying {}: {}", target_url_string, error);
            return HttpResponse::BadGateway().finish();
        },
    };

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
                "Proxing URL '{:?}' path '{}' content-type is '{:?}'",
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
            let replacement_body = match target_url.path().to_lowercase() {
                path if path.ends_with("r.css") => Some(R_CSS),
                path if path.ends_with("prism.css") => Some(PRISM_CSS),
                _ => None,
            };

            // Return the replacement resource or the real resource.
            match replacement_body {
                Some(body) => http_response_builder.body(body),
                None => http_response_builder.body(match response.bytes().await {
                    Ok(body) => body,
                    Err(error) => {
                        println!("Error {:?}", error);
                        return HttpResponse::BadGateway().finish();
                    },
                }),
            }
        },
        // Error.
        Err(error) => {
            log::info!("Error proxying {}: {}", target_url, error);
            HttpResponse::BadGateway().finish()
        },
    }
}

// Replacement for r.css.
static R_CSS: &'static str = "
body, td {
    font-size: var(--vscode-font-size);
    font-family: var(--vscode-font-family);
    color: var(--vscode-editor-foreground);
    background: var(--vscode-editor-background);
    line-height: 1.5;
}
body code,
body pre {
    color: var(--vscode-editor-foreground);
    font-size: var(--vscode-editor-font-size);
    font-family: var(--vscode-editor-font-family);
    font-weight: var(--vscode-editor-font-weight);
}
a {
    color: var(--vscode-textLink-foreground);
}
::selection {
    background: var(--vscode-editor-selectionBackground);
}
h1 {
    font-size: x-large;
}
h2 {
    font-size: x-large;
    font-weight: normal;
}
h3 {
}
h4 {
    font-style: italic;
}
h5 {
}
h6 {
    font-style: italic;
}
img.toplogo {
    max-width: 4em;
    vertical-align: middle;
}
img.arrow {
    width: 30px;
    height: 30px;
    border: 0;
}
span.acronym {
    font-size: small;
}
span.env {
    font-size: var(--vscode-editor-font-size);
    font-family: var(--vscode-editor-font-family);
}
span.file {
    font-size: var(--vscode-editor-font-size);
    font-family: var(--vscode-editor-font-family);
}
span.option {
    font-size: var(--vscode-editor-font-size);
    font-family: var(--vscode-editor-font-family);
}
span.pkg {
    font-weight: bold;
}
span.samp {
    font-size: var(--vscode-editor-font-size);
    font-family: var(--vscode-editor-font-family);
}
table p {
    margin-top: 0;
    margin-bottom: 6px;
    margin-left: 6px;
}
h3.r-arguments-title + table tr td:first-child {
    vertical-align: top;
    min-width: 24px;
    padding-right: 12px;
}
hr {
    height: 1.5px;
    border: none;
    background-color: var(--vscode-textBlockQuote-border);
}
";

// Replacement for prism.css.
static PRISM_CSS: &'static str = "
code[class*='language-'],
pre[class*='language-'] {
    color: var(--vscode-editor-foreground);
    background: none;
    text-shadow: none;
    font-family: var(--vscode-editor-font-family);
    font-size: var(--vscode-editor-font-size);
    text-align: left;
    white-space: pre;
    word-spacing: normal;
    word-break: normal;
    word-wrap: normal;
    line-height: 1.75;

    -moz-tab-size: 4;
    -o-tab-size: 4;
    tab-size: 4;

    -webkit-hyphens: none;
    -moz-hyphens: none;
    -ms-hyphens: none;
    hyphens: none;
}

@media print {
    code[class*='language-'],
    pre[class*='language-'] {
        text-shadow: none;
    }
}

pre[class*='language-'],
:not(pre) > code[class*='language-'] {
    /*background: hsl(30, 20%, 25%);*/
}

/* Code blocks */
pre[class*='language-'] {
    padding: 1em;
    margin: .5em 0;
    overflow: auto;
    border: 1.5px solid var(--vscode-textBlockQuote-border);
    border-radius: .5em;
    /*box-shadow: 1px 1px .5em black inset;*/
}

/* Inline code */
:not(pre) > code[class*='language-'] {
    padding: .15em .2em .05em;
    border-radius: .3em;
    border: .13em solid hsl(30, 20%, 40%);
    box-shadow: 1px 1px .3em -.1em black inset;
    white-space: normal;
}

.token.comment,
.token.prolog,
.token.doctype,
.token.cdata {
    color: hsl(30, 20%, 50%);
}

.token.punctuation {
    opacity: .7;
}

.token.namespace {
    opacity: .7;
}

.token.property,
.token.tag,
.token.boolean,
.token.number,
.token.constant,
.token.symbol {
    /*color: hsl(350, 40%, 70%);*/
    color: var(--vscode-positronConsole-ansiMagenta);
}

.token.selector,
.token.attr-name,
.token.string,
.token.char,
.token.builtin,
.token.inserted {
    /*color: hsl(75, 70%, 60%);*/
    color: var(--vscode-positronConsole-ansiGreen);
}

.token.operator,
.token.entity,
.token.url,
.language-css .token.string,
.style .token.string,
.token.variable {
    /*color: hsl(40, 90%, 60%);*/
    color: var(--vscode-positronConsole-ansiCyan);
}

.token.atrule,
.token.attr-value,
.token.keyword {
    /*color: hsl(350, 40%, 70%);*/
    color: var(--vscode-positronConsole-ansiMagenta);
}

.token.regex,
.token.important {
    /*color: #e90;*/
    color: var(--vscode-positronConsole-ansiBrightYellow);
}

.token.important,
.token.bold {
    font-weight: bold;
}
.token.italic {
    font-style: italic;
}

.token.entity {
    cursor: help;
}

.token.deleted {
    color: var(--vscode-positronConsole-ansiRed);
}
";
