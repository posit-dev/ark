//
// graphics_device.rs
//
// Copyright (C) 2022-2025 by Posit Software, PBC
//

// See `doc/graphics-devices.md` for documentation

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::plot_comm::PlotBackendReply;
use amalthea::comm::plot_comm::PlotBackendRequest;
use amalthea::comm::plot_comm::PlotFrontendEvent;
use amalthea::comm::plot_comm::PlotRenderFormat;
use amalthea::comm::plot_comm::PlotRenderSettings;
use amalthea::comm::plot_comm::PlotResult;
use amalthea::comm::plot_comm::PlotSize;
use amalthea::comm::plot_comm::UpdateParams;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use amalthea::wire::update_display_data::TransientValue;
use amalthea::wire::update_display_data::UpdateDisplayData;
use anyhow::anyhow;
use base64::engine::general_purpose;
use base64::Engine;
use crossbeam::channel::Select;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use libr::pDevDesc;
use libr::pGEcontext;
use libr::R_NilValue;
use libr::SEXP;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::unwrap;
use tokio::sync::mpsc::UnboundedReceiver as AsyncUnboundedReceiver;
use uuid::Uuid;

use crate::interface::RMain;
use crate::interface::SessionMode;
use crate::modules::ARK_ENVS;
use crate::r_task;

#[derive(Debug)]
pub(crate) enum GraphicsDeviceNotification {
    DidChangePlotRenderSettings(PlotRenderSettings),
}

thread_local! {
  // Safety: Set once by `RMain` on initialization
  static DEVICE_CONTEXT: RefCell<DeviceContext> = panic!("Must access `DEVICE_CONTEXT` from the R thread");
}

const POSITRON_PLOT_CHANNEL_ID: &str = "positron.plot";

// Expose thread initialization via function so we can keep the structs private
pub(crate) fn init_graphics_device(
    comm_manager_tx: Sender<CommManagerEvent>,
    iopub_tx: Sender<IOPubMessage>,
    graphics_device_rx: AsyncUnboundedReceiver<GraphicsDeviceNotification>,
) {
    DEVICE_CONTEXT.set(DeviceContext::new(comm_manager_tx, iopub_tx));

    // Launch an R thread task to process messages from the frontend
    r_task::spawn_interrupt(|| async move { process_notifications(graphics_device_rx).await });
}

async fn process_notifications(
    mut graphics_device_rx: AsyncUnboundedReceiver<GraphicsDeviceNotification>,
) {
    log::trace!("Now listening for graphics device notifications");

    loop {
        while let Some(notification) = graphics_device_rx.recv().await {
            log::trace!("Got graphics device notification: {notification:#?}");

            match notification {
                GraphicsDeviceNotification::DidChangePlotRenderSettings(plot_render_settings) => {
                    // Safety: Note that `DEVICE_CONTEXT` is accessed at
                    // interrupt time. Other methods in this file should be
                    // written in accordance and avoid causing R interrupt
                    // checks while they themselves access the device.
                    DEVICE_CONTEXT
                        .with_borrow(|ctx| ctx.prerender_settings.replace(plot_render_settings));
                },
            }
        }
    }
}

/// Wrapped callbacks of the original graphics device we shadow
#[derive(Debug, Default)]
#[allow(non_snake_case)]
struct WrappedDeviceCallbacks {
    activate: Cell<Option<unsafe extern "C-unwind" fn(pDevDesc)>>,
    deactivate: Cell<Option<unsafe extern "C-unwind" fn(pDevDesc)>>,
    holdflush: Cell<Option<unsafe extern "C-unwind" fn(pDevDesc, i32) -> i32>>,
    mode: Cell<Option<unsafe extern "C-unwind" fn(i32, pDevDesc)>>,
    newPage: Cell<Option<unsafe extern "C-unwind" fn(pGEcontext, pDevDesc)>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PlotId(String);

struct DeviceContext {
    /// Channel for sending [CommManagerEvent]s to Positron when plot events occur
    comm_manager_tx: Sender<CommManagerEvent>,

    /// Channel for sending [IOPubMessage::DisplayData] and
    /// [IOPubMessage::UpdateDisplayData] to Jupyter frontends when plot events occur
    iopub_tx: Sender<IOPubMessage>,

    /// Tracks whether the graphics device has any changes.
    ///
    /// Set to `true` if the device's `mode` ever flips to `1`, indicating some drawing
    /// has occurred. Reset back to `false` once we've processed those changes.
    has_changes: Cell<bool>,

    /// Tracks whether or not the current plot page has ever been written to.
    ///
    /// Set to `true` in the [DeviceContext::new_page] hook. Set to `false` once we've
    /// processed the changes on this page at least once.
    is_new_page: Cell<bool>,

    /// Tracks whether or not the graphics device is currently "drawing".
    ///
    /// Tracks the device's `mode`, where `mode == 1` means we are drawing,
    /// and `mode == 0` means we stop drawing.
    is_drawing: Cell<bool>,

    /// Tracks whether or not we are allowed to render a plot
    ///
    /// When a new page event occurs, or when we finish executing R code, we record the
    /// display list and send Positron a notification that we have new plot information to
    /// display. When it responds, we actually render the plot and send it back to
    /// Positron. If a user sets `dev.hold()`, then we refrain from actually responding
    /// to that render request until an equivalent call to `dev.flush()` frees us.
    ///
    /// Tracks the device's holdflush `level`, where we simplify it to mean that `level <=
    /// 0` means we can render, and `level > 0` means we cannot. Graphics devices should
    /// return `0` as the `level` when we are free to render and the level should never go
    /// negative, but we try to be extra safe.
    should_render: Cell<bool>,

    /// The ID associated with the current plot page.
    ///
    /// Used for looking up a recorded plot so we can replay it with different graphics
    /// device specifications (i.e. for Positron's Plots pane).
    id: RefCell<PlotId>,

    /// Mapping of plot ID to the communication socket used for communicating its
    /// rendered results to the frontend.
    sockets: RefCell<HashMap<PlotId, CommSocket>>,

    /// The callbacks of the wrapped device, initialized on graphics device creation
    wrapped_callbacks: WrappedDeviceCallbacks,

    /// The settings used for pre-renderings of new plots.
    prerender_settings: Cell<PlotRenderSettings>,
}

impl DeviceContext {
    fn new(comm_manager_tx: Sender<CommManagerEvent>, iopub_tx: Sender<IOPubMessage>) -> Self {
        Self {
            comm_manager_tx,
            iopub_tx,
            has_changes: Cell::new(false),
            is_new_page: Cell::new(true),
            is_drawing: Cell::new(false),
            should_render: Cell::new(true),
            id: RefCell::new(Self::new_id()),
            sockets: RefCell::new(HashMap::new()),
            wrapped_callbacks: WrappedDeviceCallbacks::default(),
            prerender_settings: Cell::new(PlotRenderSettings {
                size: PlotSize {
                    width: 640,
                    height: 400,
                },
                pixel_ratio: 1.,
                format: PlotRenderFormat::Png,
            }),
        }
    }

    /// Create a new id for this new plot page (from Positron's perspective)
    /// and note that this is a new page
    fn new_positron_page(&self) {
        self.is_new_page.replace(true);
        self.id.replace(Self::new_id());
    }

    /// Should plot events be sent over [CommSocket]s to the frontend?
    ///
    /// This allows plots to be dynamically resized by their `id`. Only possible if the UI
    /// comm is connected (i.e. we are connected to Positron) and if we are in
    /// [SessionMode::Console] mode.
    fn should_use_dynamic_plots(&self) -> bool {
        RMain::with(|main| {
            main.is_ui_comm_connected() && main.session_mode() == SessionMode::Console
        })
    }

    /// Deactivation hook
    ///
    /// We process any changes here before fully deactivating, ensuring that
    /// we record the current display list before a different device takes control,
    /// because that new device may wipe the display list.
    ///
    /// For example, running this all in one chunk should plot `1:10` at the end of the
    /// chunk as long as we record the `1:10` plot before we switch to the png device.
    ///
    /// ```r
    /// plot(1:10)
    /// grDevices::png()
    /// plot(1:20)
    /// dev.off()
    /// ```
    ///
    /// That's also exactly what happens here with ggsave
    ///
    /// ```r
    /// library(ggplot2)
    /// p <- ggplot(mtcars, aes(x = wt, y = mpg)) + geom_point()
    /// p
    /// ggsave("temp.png", p)
    /// ```
    #[tracing::instrument(level = "trace", skip_all)]
    fn hook_deactivate(&self) {
        self.process_changes();
    }

    #[tracing::instrument(level = "trace", skip_all, fields(level = %level))]
    fn hook_holdflush(&self, level: i32) {
        // Be extra safe and check `level <= 0` rather than just `level == 0` in case
        // our shadowed device returns a negative `level`
        self.should_render.replace(level <= 0);
    }

    #[tracing::instrument(level = "trace", skip_all, fields(mode = %mode))]
    fn hook_mode(&self, mode: i32) {
        let is_drawing = mode != 0;
        self.is_drawing.replace(is_drawing);
        let old_has_changes = self.has_changes.get();
        self.has_changes.replace(old_has_changes || is_drawing);
    }

    /// Hook applied when starting a new page
    ///
    /// Notably this hook is called by the R graphics system after the display list
    /// of the old page has been cleared, so it is too late to try and record here.
    /// If you are looking for where we record before the page is advanced, look to
    /// [ps_graphics_before_plot_new()] instead.
    #[tracing::instrument(level = "trace", skip_all)]
    fn hook_new_page(&self) {
        self.new_positron_page()
    }

    fn id(&self) -> PlotId {
        // Refcell Safety: Short borrows in the file.
        self.id.borrow().clone()
    }

    fn new_id() -> PlotId {
        PlotId(Uuid::new_v4().to_string())
    }

    /// Process outstanding RPC requests received from Positron
    ///
    /// At idle time we loop through our set of plot channels and check if Positron has
    /// responded on any of them stating that it is ready for us to replay and render
    /// the actual plot, and then send back the bytes that represent that plot.
    ///
    /// Note that we only send back rendered plots at idle time. This means that if you
    /// do something like:
    ///
    /// ```r
    /// for (i in 1:5) {
    ///   plot(i)
    ///   Sys.sleep(1)
    /// }
    /// ```
    ///
    /// Then it goes something like this:
    /// - At each new page event we tell Positron there we have a new plot for it
    /// - Positron sets up 5 blank plot windows and sends back an RPC requesting the plot
    ///   data
    /// - AFTER the entire for loop has finished and we hit idle time, we drop into
    ///   `process_rpc_requests()` and render all 5 plots at once
    ///
    /// Practically this seems okay, it is just something to keep in mind.
    #[tracing::instrument(level = "trace", skip_all)]
    fn process_rpc_requests(&self) {
        // Don't try to render a plot if we're currently drawing.
        if self.is_drawing.get() {
            log::trace!("Refusing to render due to `is_drawing`");
            return;
        }

        // Don't try to render a plot if someone is asking us not to, i.e. `dev.hold()`
        if !self.should_render.get() {
            log::trace!("Refusing to render due to `should_render`");
            return;
        }

        // Collect existing sockets into a vector of tuples.
        // Necessary for handling Select in a clean way.
        let sockets = {
            // Refcell Safety: Clone the hashmap so we don't hold a reference for too long
            let sockets = self.sockets.borrow().clone();
            sockets.into_iter().collect::<Vec<_>>()
        };

        // Dynamically load all incoming channels within the sockets into a single `Select`
        let mut select = Select::new();
        for (_id, sockets) in sockets.iter() {
            select.recv(&sockets.incoming_rx);
        }

        // Check for incoming plot render requests.
        // Totally possible to have >1 requests pending, especially if we've plotted
        // multiple things in a single chunk of R code. The `Err` case is likely just
        // that no channels have any messages, so we don't log in that case.
        while let Ok(selection) = select.try_select() {
            let socket = sockets
                .get(selection.index())
                .expect("Socket should exist for the selection index");
            let id = &socket.0;
            let socket = &socket.1;

            // Receive on the "selected" channel
            let message = match selection.recv(&socket.incoming_rx) {
                Ok(message) => message,
                Err(error) => {
                    // If the channel is disconnected, log and remove it so we don't try
                    // and `recv()` on it ever again
                    log::error!("{error:?}");
                    // Refcell Safety: Short borrows in the file.
                    self.sockets.borrow_mut().remove(id);

                    // Process remaining messages. Safe to do because we have
                    // removed the `DeviceContext`'s copy off the sockets but we
                    // are working through our own copy of them.
                    continue;
                },
            };

            match message {
                CommMsg::Rpc(_, _) => {
                    log::trace!("Handling `RPC` for plot `id` {id}");
                    socket.handle_request(message, |req| self.handle_rpc(req, id));
                },

                // Note that ideally this handler should be invoked before we
                // check for `should_render`. I.e. we should acknowledge a plot
                // has been closed on the frontend side even when `dev.hold()`
                // is active. Doing so would require some more careful
                // bookkeeping of the state though, and since this is a very
                // unlikely sequence of action nothing really bad happens with
                // the current approach, we decided to keep handling here.
                CommMsg::Close => {
                    log::trace!("Handling `Close` for plot `id` {id}");
                    self.close_plot(id)
                },

                message => {
                    log::error!("Received unexpected comm message for plot `id` {id}: {message:?}")
                },
            }
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn handle_rpc(
        &self,
        message: PlotBackendRequest,
        id: &PlotId,
    ) -> anyhow::Result<PlotBackendReply> {
        match message {
            PlotBackendRequest::GetIntrinsicSize => {
                log::trace!("PlotBackendRequest::GetIntrinsicSize");
                Ok(PlotBackendReply::GetIntrinsicSizeReply(None))
            },
            PlotBackendRequest::Render(plot_meta) => {
                log::trace!("PlotBackendRequest::Render");

                let size = unwrap!(plot_meta.size, None => {
                    return Err(anyhow!("Intrinsically sized plots are not yet supported."));
                });

                let settings = PlotRenderSettings {
                    size: PlotSize {
                        width: size.width,
                        height: size.height,
                    },
                    pixel_ratio: plot_meta.pixel_ratio,
                    format: plot_meta.format,
                };

                let data = self.render_plot(&id, &settings)?;
                let mime_type = Self::get_mime_type(&plot_meta.format);

                Ok(PlotBackendReply::RenderReply(PlotResult {
                    data: data.to_string(),
                    mime_type: mime_type.to_string(),
                    settings: Some(settings),
                }))
            },
        }
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn close_plot(&self, id: &PlotId) {
        // RefCell safety: Short borrows in the file
        self.sockets.borrow_mut().remove(id);

        // The plot data is stored at R level. Assumes we're called on the R
        // thread at idle time so there's no race issues (see
        // `on_process_idle_events()`).
        if let Err(err) = RFunction::from("remove_recording")
            .param("id", id)
            .call_in(ARK_ENVS.positron_ns)
        {
            log::error!("Can't clean up plot (id: {id}): {err:?}");
        }

        // If the currently active plot is closed, advance to a new Positron page
        // See https://github.com/posit-dev/positron/issues/6702.
        if *self.id.borrow() == *id {
            self.new_positron_page();
        }
    }

    fn get_mime_type(format: &PlotRenderFormat) -> String {
        match format {
            PlotRenderFormat::Png => "image/png".to_string(),
            PlotRenderFormat::Svg => "image/svg+xml".to_string(),
            PlotRenderFormat::Pdf => "application/pdf".to_string(),
            PlotRenderFormat::Jpeg => "image/jpeg".to_string(),
            PlotRenderFormat::Tiff => "image/tiff".to_string(),
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn process_changes(&self) {
        let id = self.id();

        if !self.has_changes.replace(false) {
            log::trace!("No changes to process for plot `id` {id}");
            return;
        }

        log::trace!("Processing changes for plot `id` {id}");

        // Record the changes so we can replay them when Positron asks us for them.
        // Recording here overrides an existing recording for `id` if something has
        // changed between then and now, which is what we want, for example, we want
        // it when running this line by line:
        //
        // ```r
        // par(mfrow = c(2, 1))
        // plot(1) # Should get recorded with `id1`
        // plot(2) # Should record and overwrite `id1` because no new_page has been requested
        // ```
        Self::record_plot(&id);

        if self.is_new_page.replace(false) {
            self.process_new_plot(&id);
        } else {
            self.process_update_plot(&id);
        }
    }

    fn process_new_plot(&self, id: &PlotId) {
        if self.should_use_dynamic_plots() {
            self.process_new_plot_positron(id);
        } else {
            self.process_new_plot_jupyter_protocol(id);
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_new_plot_positron(&self, id: &PlotId) {
        log::trace!("Notifying Positron of new plot");

        // Let Positron know that we just created a new plot.
        let socket = CommSocket::new(
            CommInitiator::BackEnd,
            id.to_string(),
            POSITRON_PLOT_CHANNEL_ID.to_string(),
        );

        let settings = self.prerender_settings.get();

        // Prepare a pre-rendering of the plot so Positron has something to display immediately
        let data = match self.render_plot(id, &settings) {
            Ok(pre_render) => {
                let mime_type = Self::get_mime_type(&PlotRenderFormat::Png);

                let pre_render = PlotResult {
                    data: pre_render.to_string(),
                    mime_type: mime_type.to_string(),
                    settings: Some(settings),
                };

                serde_json::json!({ "pre_render": pre_render })
            },
            Err(err) => {
                log::warn!("Can't pre-render plot: {err:?}");
                serde_json::Value::Null
            },
        };

        let event = CommManagerEvent::Opened(socket.clone(), data);
        if let Err(error) = self.comm_manager_tx.send(event) {
            log::error!("{error:?}");
        }

        // Save our new socket.
        // Refcell Safety: Short borrows in the file.
        self.sockets.borrow_mut().insert(id.clone(), socket);
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_new_plot_jupyter_protocol(&self, id: &PlotId) {
        log::trace!("Notifying Jupyter frontend of new plot");

        let data = unwrap!(self.create_display_data_plot(id), Err(error) => {
            log::error!("Failed to create plot due to: {error}.");
            return;
        });

        let metadata = json!({});

        // For `DisplayData`, the `transient` slot is a simple `Value`,
        // but we can use the `TransientValue` required by `UpdateDisplayData`
        // to structure this object since we pass through a `display_id` in
        // this particular case.
        let transient = TransientValue {
            display_id: id.to_string(),
            data: None,
        };
        let transient = serde_json::to_value(transient).unwrap();

        log::info!("Sending display data to IOPub.");

        self.iopub_tx
            .send(IOPubMessage::DisplayData(DisplayData {
                data,
                metadata,
                transient,
            }))
            .or_log_warning(&format!("Could not publish display data on IOPub."));
    }

    fn process_update_plot(&self, id: &PlotId) {
        if self.should_use_dynamic_plots() {
            self.process_update_plot_positron(id);
        } else {
            self.process_update_plot_jupyter_protocol(id);
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_update_plot_positron(&self, id: &PlotId) {
        log::trace!("Notifying Positron of plot update");

        // Refcell Safety: Make sure not to call other methods from this whole block.
        let sockets = self.sockets.borrow();

        // Find our socket
        let socket = unwrap!(sockets.get(id), None => {
            // If socket doesn't exist, bail, nothing to update (should be rare, likely a bug?)
            log::error!("Can't find socket to update with id: {id}.");
            return;
        });

        // Create a pre-rendering of the updated plot
        let settings = self.prerender_settings.get();
        let update_params = match self.render_plot(id, &settings) {
            Ok(pre_render) => {
                let mime_type = Self::get_mime_type(&settings.format);

                let pre_render = PlotResult {
                    data: pre_render.to_string(),
                    mime_type: mime_type.to_string(),
                    settings: Some(settings),
                };

                UpdateParams {
                    pre_render: Some(pre_render),
                }
            },
            Err(err) => {
                log::warn!("Can't pre-render plot update: {err:?}");
                UpdateParams { pre_render: None }
            },
        };

        let value = serde_json::to_value(PlotFrontendEvent::Update(update_params)).unwrap();

        // Tell Positron we have an updated plot with optional pre-rendering
        socket
            .outgoing_tx
            .send(CommMsg::Data(value))
            .or_log_error("Failed to send update message for id {id}.");
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_update_plot_jupyter_protocol(&self, id: &PlotId) {
        log::trace!("Notifying Jupyter frontend of plot update");

        let data = unwrap!(self.create_display_data_plot(id), Err(error) => {
            log::error!("Failed to create plot due to: {error}.");
            return;
        });

        let metadata = json!({});

        let transient = TransientValue {
            display_id: id.to_string(),
            data: None,
        };

        log::info!("Sending update display data to IOPub for `id` {id}.");

        self.iopub_tx
            .send(IOPubMessage::UpdateDisplayData(UpdateDisplayData {
                data,
                metadata,
                transient,
            }))
            .or_log_warning(&format!("Could not publish update display data on IOPub."));
    }

    fn create_display_data_plot(&self, id: &PlotId) -> Result<serde_json::Value, anyhow::Error> {
        // TODO: Take these from R global options? Like `ark.plot.width`?
        let settings = PlotRenderSettings {
            size: PlotSize {
                width: 800,
                height: 600,
            },
            pixel_ratio: 1.0,
            format: PlotRenderFormat::Png,
        };

        let data = unwrap!(self.render_plot(id, &settings), Err(error) => {
            return Err(anyhow!("Failed to render plot with id {id} due to: {error}."));
        });

        let mut map = serde_json::Map::new();
        map.insert("image/png".to_string(), serde_json::to_value(data).unwrap());

        Ok(serde_json::Value::Object(map))
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn render_plot(&self, id: &PlotId, settings: &PlotRenderSettings) -> anyhow::Result<String> {
        log::trace!("Rendering plot");

        let image_path = r_task(|| unsafe {
            RFunction::from(".ps.graphics.render_plot_from_recording")
                .param("id", id)
                .param("width", RObject::try_from(settings.size.width)?)
                .param("height", RObject::try_from(settings.size.height)?)
                .param("pixel_ratio", settings.pixel_ratio)
                .param("format", settings.format.to_string())
                .call()?
                .to::<String>()
        });

        let image_path = match image_path {
            Ok(image_path) => image_path,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "Failed to render plot with `id` {id} due to: {error}."
                ))
            },
        };

        log::trace!("Rendered plot to {image_path}");

        // Read contents into bytes.
        let conn = File::open(image_path)?;
        let mut reader = BufReader::new(conn);

        let mut buffer = vec![];
        reader.read_to_end(&mut buffer)?;

        // what an odd interface
        let data = general_purpose::STANDARD_NO_PAD.encode(buffer);

        Ok(data)
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn record_plot(id: &PlotId) -> bool {
        log::trace!("Recording plot");

        let result = RFunction::from(".ps.graphics.record_plot")
            .param("id", id)
            .call();

        match result {
            Ok(_) => {
                log::trace!("Recorded plot");
                true
            },
            Err(error) => {
                log::error!("Failed to record plot: {error:?}");
                false
            },
        }
    }
}

// TODO: This macro needs to be updated every time we introduce support
// for a new graphics device. Is there a better way?
macro_rules! with_device {
    ($ge_value:expr, | $ge_name:ident, $name:ident | $block:block) => {{
        let version = libr::R_GE_getVersion();
        if version == 13 {
            let $ge_name = $ge_value as *mut libr::GEDevDescVersion13;
            let $name = (*$ge_name).dev;
            $block;
        } else if version == 14 {
            let $ge_name = $ge_value as *mut libr::GEDevDescVersion14;
            let $name = (*$ge_name).dev;
            $block;
        } else if version == 15 {
            let $ge_name = $ge_value as *mut libr::GEDevDescVersion15;
            let $name = (*$ge_name).dev;
            $block;
        } else if version == 16 {
            let $ge_name = $ge_value as *mut libr::GEDevDescVersion16;
            let $name = (*$ge_name).dev;
            $block;
        } else if version == 17 {
            let $ge_name = $ge_value as *mut libr::GEDevDescVersion17;
            let $name = (*$ge_name).dev;
            $block;
        } else {
            panic!(
                "R graphics engine version {} is not supported by this version of Positron.",
                version
            )
        };
    }};
}

impl Display for PlotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<PlotId> for RObject {
    fn from(value: PlotId) -> Self {
        RObject::from(value.0)
    }
}

impl From<&PlotId> for RObject {
    fn from(value: &PlotId) -> Self {
        RObject::from(value.0.as_str())
    }
}

/// Hook applied at idle time (`R_ProcessEvents()` time) to process any outstanding
/// RPC requests from Positron
///
/// This is called a lot, so we don't trace log each entry
#[tracing::instrument(level = "trace", skip_all)]
pub(crate) fn on_process_idle_events() {
    DEVICE_CONTEXT.with_borrow(|cell| cell.process_rpc_requests());
}

/// Hook applied after a code chunk has finished executing
///
/// Not an official graphics device hook, instead we run this manually after
/// completing execution of a chunk of R code.
///
/// This is particularly useful for recording "partial" states within a single
/// page, for example:
///
/// ```r
/// # Run this line by line
/// par(mfrow = c(2, 1))
/// plot(1:10)
/// ```
///
/// After `plot(1:10)`, we've only plotted 1 of 2 potential plots on the page,
/// but we can still render this intermediate state and show it to the user until
/// they add more plots or advance to another new page.
#[tracing::instrument(level = "trace", skip_all)]
pub(crate) fn on_did_execute_request() {
    log::trace!("Entering on_did_execute_request");
    DEVICE_CONTEXT.with_borrow(|cell| cell.process_changes());
}

/// Activation callback
///
/// Only used for logging
///
/// NOTE: May be called during [DeviceContext::render_plot], since this is done by
/// copying the graphics display list to a new plot device, and then closing that device.
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_activate(dev: pDevDesc) {
    log::trace!("Entering callback_activate");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell.wrapped_callbacks.activate.get() {
            callback(dev);
        }
    });
}

/// Deactivation callback
///
/// NOTE: May be called during [DeviceContext::render_plot], since this is done by
/// copying the graphics display list to a new plot device, and then closing that device.
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_deactivate(dev: pDevDesc) {
    log::trace!("Entering callback_deactivate");

    DEVICE_CONTEXT.with_borrow(|cell| {
        // We run our hook first to record before we deactivate the underlying device,
        // in case device deactivation messes with the display list
        cell.hook_deactivate();
        if let Some(callback) = cell.wrapped_callbacks.deactivate.get() {
            callback(dev);
        }
    });
}

#[tracing::instrument(level = "trace", skip_all, fields(level_delta = %level_delta))]
unsafe extern "C-unwind" fn callback_holdflush(dev: pDevDesc, level_delta: i32) -> i32 {
    log::trace!("Entering callback_holdflush");

    DEVICE_CONTEXT.with_borrow(|cell| {
        // If our wrapped device has a `holdflush()` method, we rely on it to apply
        // the `level_delta` (typically `+1` or `-1`) and return the new level. Otherwise
        // we follow the lead of `devholdflush()` in R and use a resolved `level` of `0`.
        // Notably, `grDevices::png()` with a Cairo backend does not have a holdflush
        // hook.
        // https://github.com/wch/r-source/blob/8cebcc0a5d99890839e5171f398da643d858dcca/src/library/grDevices/src/devices.c#L129-L138
        let level = match cell.wrapped_callbacks.holdflush.get() {
            Some(callback) => {
                let level = callback(dev, level_delta);
                log::trace!("Using resolved holdflush level from wrapped callback: {level}");
                level
            },
            None => {
                let level = 0;
                log::trace!("Using default holdflush level: {level}");
                level
            },
        };
        cell.hook_holdflush(level);
        level
    })
}

// mode = 0, graphics off
// mode = 1, graphics on
// mode = 2, graphical input on (ignored by most drivers)
#[tracing::instrument(level = "trace", skip_all, fields(mode = %mode))]
unsafe extern "C-unwind" fn callback_mode(mode: i32, dev: pDevDesc) {
    log::trace!("Entering callback_mode");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell.wrapped_callbacks.mode.get() {
            callback(mode, dev);
        }
        cell.hook_mode(mode);
    });
}

#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_new_page(dd: pGEcontext, dev: pDevDesc) {
    log::trace!("Entering callback_new_page");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell.wrapped_callbacks.newPage.get() {
            callback(dd, dev);
        }
        cell.hook_new_page();
    });
}

unsafe fn ps_graphics_device_impl() -> anyhow::Result<SEXP> {
    // TODO: Don't allow creation of more than one graphics device.

    // Create the graphics device.
    RFunction::from(".ps.graphics.create_device").call()?;

    // Get reference to current device (opaque pointer)
    let ge_device = libr::GEcurrentDevice();

    // Initialize display list (needed for copying of plots)
    // (Called on opaque pointer, because that matches the function signature.
    // Pointer specialization is done below, at which point we can access and set
    // `displayListOn` too)
    libr::GEinitDisplayList(ge_device);

    // Get a specialized versioned pointer from our opaque one so we can initialize our
    // `wrapped_callbacks`
    with_device!(ge_device, |ge_device, device| {
        (*ge_device).displayListOn = 1;

        DEVICE_CONTEXT.with_borrow(|cell| {
            let wrapped_callbacks = &cell.wrapped_callbacks;

            // Safety: The callbacks are stored in simple cells.

            wrapped_callbacks.activate.replace((*device).activate);
            (*device).activate = Some(callback_activate);

            wrapped_callbacks.deactivate.replace((*device).deactivate);
            (*device).deactivate = Some(callback_deactivate);

            wrapped_callbacks.holdflush.replace((*device).holdflush);
            (*device).holdflush = Some(callback_holdflush);

            wrapped_callbacks.mode.replace((*device).mode);
            (*device).mode = Some(callback_mode);

            wrapped_callbacks.newPage.replace((*device).newPage);
            (*device).newPage = Some(callback_new_page);
        });
    });

    Ok(R_NilValue)
}

#[tracing::instrument(level = "trace", skip_all)]
#[harp::register]
unsafe extern "C-unwind" fn ps_graphics_device() -> anyhow::Result<SEXP> {
    log::trace!("Initializing Ark graphics device");
    ps_graphics_device_impl().or_else(|error| {
        log::error!("{error:?}");
        Ok(R_NilValue)
    })
}

/// Hook applied by `plot.new()` and `grid::grid.newpage()`
///
/// The timing of this hook is particularly important. If we advance to the new page, then
/// when we drop into our [DeviceContext::new_page] hook it will be too late for us to
/// record any changes because the display list we record gets cleared before our hook is
/// called.
///
/// It is also worth noting that we don't always drop into our [DeviceContext::new_page]
/// hook after a call to `plot.new()` (and, therefore, after this hook is called). With
/// this:
///
/// ```r
/// par(mfrow = c(2, 1))
/// plot(1:10)
/// plot(2:20)
/// ```
///
/// When running `plot(2:20)` the internal `plot.new()` will trigger this hook, but will
/// not trigger a [DeviceContext::new_page] hook. That's correct, as it gives us a chance
/// to record the intermediate plot, and then after `plot(2:20)` finishes it overwrites
/// that intermediate plot since we are still on the same plot page with the same plot
/// `id`.
#[tracing::instrument(level = "trace", skip_all)]
#[harp::register]
unsafe extern "C-unwind" fn ps_graphics_before_plot_new(_name: SEXP) -> anyhow::Result<SEXP> {
    log::trace!("Entering ps_graphics_before_plot_new");

    DEVICE_CONTEXT.with_borrow(|cell| {
        // Process changes related to the last plot before opening a new page.
        // Particularly important if we make multiple plots in a single chunk.
        cell.process_changes();
    });

    Ok(harp::r_null())
}
