//
// r_help.rs
//
// Copyright (C) 2023-2026 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::help_comm::HelpBackendReply;
use amalthea::comm::help_comm::HelpBackendRequest;
use amalthea::comm::help_comm::HelpFrontendEvent;
use amalthea::comm::help_comm::ShowHelpKind;
use amalthea::comm::help_comm::ShowHelpParams;
use anyhow::anyhow;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::RObject;
use libr::R_NilValue;
use libr::SEXP;

use crate::comm_handler::handle_rpc_request;
use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::console;
use crate::console::Console;
use crate::help::message::HelpEvent;
use crate::help::message::ShowHelpUrlKind;
use crate::help::message::ShowHelpUrlParams;
use crate::help_proxy;
use crate::methods::ArkGenerics;

pub const HELP_COMM_NAME: &str = "positron.help";

/// Ports for the R help server and our proxy, recorded on `Console` once both
/// are running.
#[derive(Clone, Copy)]
pub struct HelpPorts {
    pub r_port: u16,
    pub proxy_port: u16,
}

/// The R Help handler (together with the help proxy) provides the server side
/// of Positron's Help panel.
///
/// The server and proxy ports live on `Console` as `help_ports`, not here, so
/// the `browseURL()` hook can reach them through `Console::get()`. What this
/// handler does own is the proxy drop guard, which shuts the proxy down when
/// the help comm closes.
#[derive(Debug, Default)]
pub struct RHelp {
    /// Drop guard to stop the help proxy server on teardown.
    proxy: Option<help_proxy::ProxyHandle>,
}

impl RHelp {
    /// Public associated function so that callers can cheaply check if a url is
    /// a help url without going through the handler (like in the case of
    /// `browseURL()`).
    pub fn is_help_url(url: &str, port: u16) -> bool {
        let prefix = Self::help_url_prefix(port);
        url.starts_with(prefix.as_str())
    }

    fn help_url_prefix(port: u16) -> String {
        format!("http://127.0.0.1:{port}/")
    }

    fn handle_rpc(&self, message: HelpBackendRequest) -> anyhow::Result<HelpBackendReply> {
        match message {
            HelpBackendRequest::ShowHelpTopic(topic) => {
                // Look up the help topic and attempt to show it; this returns a
                // boolean indicating whether the topic was found.
                match self.show_help_topic(topic.topic.clone()) {
                    Ok(found) => Ok(HelpBackendReply::ShowHelpTopicReply(found)),
                    Err(err) => Err(err),
                }
            },
        }
    }

    /// Translate a help event into a frontend `ShowHelp` message and send it on
    /// the comm's outgoing channel.
    pub fn handle_event(
        event: HelpEvent,
        ctx: &CommHandlerContext,
        r_port: u16,
        proxy_port: u16,
    ) -> anyhow::Result<()> {
        log::trace!("{event:#?}");
        match event {
            HelpEvent::ShowHelpUrl(params) => {
                Self::handle_show_help_url(params, ctx, r_port, proxy_port)
            },
        }
    }

    /// Shows a help URL by sending a message to the frontend. We expect that any URL
    /// coming through here has already been verified to look like a help URL with
    /// `is_help_url()`, so if we get an unexpected prefix, that's an error.
    fn handle_show_help_url(
        params: ShowHelpUrlParams,
        ctx: &CommHandlerContext,
        r_port: u16,
        proxy_port: u16,
    ) -> anyhow::Result<()> {
        let url = params.url.clone();

        let url = match params.kind {
            ShowHelpUrlKind::HelpProxy => {
                if !Self::is_help_url(url.as_str(), r_port) {
                    let prefix = Self::help_url_prefix(r_port);
                    return Err(anyhow!(
                        "Help URL '{url}' doesn't have expected prefix '{prefix}'."
                    ));
                }

                // Re-direct the help event to our help proxy server.
                let r_prefix = Self::help_url_prefix(r_port);
                let proxy_prefix = Self::help_url_prefix(proxy_port);

                url.replace(r_prefix.as_str(), proxy_prefix.as_str())
            },
            ShowHelpUrlKind::External => {
                // The URL is not a help URL; just use it as-is.
                url
            },
        };

        log::trace!(
            "Sending frontend event `ShowHelp` with R url '{}' and proxy url '{}'",
            params.url,
            url
        );

        let msg = HelpFrontendEvent::ShowHelp(ShowHelpParams {
            content: url,
            kind: ShowHelpKind::Url,
            focus: true,
        });
        ctx.send_event(&msg);

        Ok(())
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn show_help_topic(&self, topic: String) -> anyhow::Result<bool> {
        let topic = HelpTopic::parse(topic);

        let found = match topic {
            HelpTopic::Simple(symbol) => {
                // Try evaluating the help handler first and then fall back to
                // the default help topic display function.
                if let Ok(Some(result)) = Self::custom_help_handler(symbol.clone()) {
                    return Ok(result);
                }

                RFunction::from(".ps.help.showHelpTopic")
                    .add(symbol)
                    .call()?
                    .to::<bool>()?
            },
            HelpTopic::Expression(expression) => {
                // For expressions, we have to use the help handler.
                // If that fails there's no fallback.
                match Self::custom_help_handler(expression) {
                    Ok(Some(result)) => result,
                    // No method found
                    Ok(None) => false,
                    // Error during evaluation
                    Err(err) => return Err(err),
                }
            },
        };

        Ok(found)
    }

    // Tries calling a custom help handler defined as an ark method.
    fn custom_help_handler(topic: String) -> anyhow::Result<Option<bool>> {
        let env = console::selected_env();

        let obj = match harp::parse_eval0(topic.as_str(), env.sexp) {
            Ok(obj) => obj,
            Err(err) => {
                // Could not parse/eval the topic; no custom handler.
                log::warn!(
                    "Could not parse/eval help topic expression '{}': {:?}",
                    topic,
                    err
                );
                return Ok(None);
            },
        };

        let handler: Option<RObject> =
            ArkGenerics::HelpGetHandler.try_dispatch(obj.sexp, vec![])?;

        if let Some(handler) = handler {
            let mut fun = RFunction::new_inlined(handler);
            match fun.call_in(env.sexp) {
                Err(err) => {
                    log::error!("Error calling help handler: {err:?}");
                    return Err(anyhow!("Error calling help handler: {err:?}"));
                },
                Ok(result) => {
                    return Ok(Some(result.try_into()?));
                },
            }
        }

        Ok(None)
    }

    pub fn start_or_reconnect_to_help_server() -> harp::Result<u16> {
        // Start the R help server.
        // If it is already started, it just returns the preexisting port number.
        RFunction::from(".ps.help.startOrReconnectToHelpServer")
            .call()
            .and_then(|x| x.try_into())
    }
}

impl CommHandler for RHelp {
    fn handle_open(&mut self, _ctx: &CommHandlerContext) {
        // Start the R help server and proxy on the R thread, then record their
        // ports on `Console` so help URLs can be recognized and rewritten. If
        // either fails to start, we leave the ports unset and help stays inert.
        let r_port = match Self::start_or_reconnect_to_help_server() {
            Ok(port) => port,
            Err(err) => {
                log::error!("Could not start R help server: {err:?}");
                return;
            },
        };
        log::info!("R help server listening on port {r_port}");

        let (proxy_port, proxy) = match help_proxy::start(r_port) {
            Ok(proxy) => proxy,
            Err(err) => {
                log::error!("Could not start R help proxy server: {err:?}");
                return;
            },
        };

        self.proxy = Some(proxy);
        Console::get_mut().set_help_ports(r_port, proxy_port);
    }

    fn handle_msg(&mut self, msg: CommMsg, ctx: &CommHandlerContext) {
        handle_rpc_request(&ctx.outgoing_tx, HELP_COMM_NAME, msg, |req| {
            self.handle_rpc(req)
        });
    }
}

enum HelpTopic {
    // no obvious expression syntax — e.g. "abs", "base::abs"
    Simple(String),
    // contains expression syntax — e.g. "tensorflow::tf$abs", "model@coef"
    // such that there will never exist a help topic with that name
    Expression(String),
}

impl HelpTopic {
    pub fn parse(topic: String) -> Self {
        if topic.contains('$') || topic.contains('@') {
            Self::Expression(topic)
        } else {
            Self::Simple(topic)
        }
    }
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_help_browse_external_url(
    url: SEXP,
) -> Result<SEXP, anyhow::Error> {
    Console::get().send_help_event(HelpEvent::ShowHelpUrl(ShowHelpUrlParams {
        url: RObject::view(url).to::<String>()?,
        kind: ShowHelpUrlKind::External,
    }))?;

    Ok(R_NilValue)
}
