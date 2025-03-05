//
// graphics_device.rs
//
// Copyright (C) 2022-2025 by Posit Software, PBC
//

// See `doc/graphics-devices.md` for documentation

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CString;
use std::fmt::Display;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::plot_comm::PlotBackendReply;
use amalthea::comm::plot_comm::PlotBackendRequest;
use amalthea::comm::plot_comm::PlotFrontendEvent;
use amalthea::comm::plot_comm::PlotResult;
use amalthea::comm::plot_comm::RenderFormat;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use amalthea::wire::update_display_data::TransientValue;
use amalthea::wire::update_display_data::UpdateDisplayData;
use anyhow::bail;
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
use uuid::Uuid;

use crate::interface::RMain;
use crate::interface::SessionMode;
use crate::r_task;

thread_local! {
  // Safety: Set once by `RMain` on initialization
  static DEVICE_CONTEXT: RefCell<DeviceContext> = panic!("Must access `DEVICE_CONTEXT` from the R thread");
}

const POSITRON_PLOT_CHANNEL_ID: &str = "positron.plot";

// Expose thread initialization via function so we can keep the structs private
pub(crate) fn init_graphics_device(
    comm_manager_tx: Sender<CommManagerEvent>,
    iopub_tx: Sender<IOPubMessage>,
) {
    DEVICE_CONTEXT.set(DeviceContext::new(comm_manager_tx, iopub_tx))
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
    /// Tracks the device's `holdflush` flag, where we simplify it to mean that `holdflush
    /// == 0` means we can render, and `holdflush > 0` means we cannot. This flag
    /// technically represents a stack of hold levels, but that doesn't affect us.
    should_render: Cell<bool>,

    /// The ID associated with the current plot page.
    ///
    /// Used for looking up a recorded plot so we can replay it with different graphics
    /// device specifications (i.e. for Positron's Plots pane).
    id: RefCell<PlotId>,

    /// Mapping of plot ID to the communication socket used for communicating its
    /// rendered results to the frontend.
    sockets: RefCell<HashMap<PlotId, CommSocket>>,

    device: libr::DevDescVersion16,
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
            device: new_device(),
        }
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

    #[tracing::instrument(level = "trace", skip_all, fields(holdflush = %holdflush))]
    fn hook_holdflush(&self, holdflush: i32) {
        //self.should_render.replace(holdflush == 0);
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
        // Create a new id for this new plot page and note that this is a new page
        self.id.replace(Self::new_id());
        self.is_new_page.replace(true);
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
            let id = unsafe { &sockets.get_unchecked(selection.index()).0 };
            let socket = unsafe { &sockets.get_unchecked(selection.index()).1 };

            // Receive on the "selected" channel
            let message = match selection.recv(&socket.incoming_rx) {
                Ok(message) => message,
                Err(error) => {
                    log::error!("{error:?}");
                    return;
                },
            };

            log::trace!("Handling RPC for plot `id` {id}");
            socket.handle_request(message, |req| self.handle_rpc(req, id));
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
                    bail!("Intrinsically sized plots are not yet supported.");
                });

                let data = self.render_plot(
                    &id,
                    size.width,
                    size.height,
                    plot_meta.pixel_ratio,
                    &plot_meta.format,
                )?;

                let mime_type = Self::get_mime_type(&plot_meta.format);

                Ok(PlotBackendReply::RenderReply(PlotResult {
                    data: data.to_string(),
                    mime_type: mime_type.to_string(),
                }))
            },
        }
    }

    fn get_mime_type(format: &RenderFormat) -> String {
        match format {
            RenderFormat::Png => "image/png".to_string(),
            RenderFormat::Svg => "image/svg+xml".to_string(),
            RenderFormat::Pdf => "application/pdf".to_string(),
            RenderFormat::Jpeg => "image/jpeg".to_string(),
            RenderFormat::Tiff => "image/tiff".to_string(),
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

        let event = CommManagerEvent::Opened(socket.clone(), serde_json::Value::Null);
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

        let value = serde_json::to_value(PlotFrontendEvent::Update).unwrap();

        // Tell Positron we have an updated plot that it should request a rerender for
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
        let width = 800;
        let height = 600;
        let pixel_ratio = 1.0;
        let format = RenderFormat::Png;

        let data = unwrap!(self.render_plot(id, width, height, pixel_ratio, &format), Err(error) => {
            bail!("Failed to render plot with id {id} due to: {error}.");
        });

        let mut map = serde_json::Map::new();
        map.insert("image/png".to_string(), serde_json::to_value(data).unwrap());

        Ok(serde_json::Value::Object(map))
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id, width = %width, height = %height, pixel_ratio = %pixel_ratio, format = %format))]
    fn render_plot(
        &self,
        id: &PlotId,
        width: i64,
        height: i64,
        pixel_ratio: f64,
        format: &RenderFormat,
    ) -> anyhow::Result<String> {
        log::trace!("Rendering plot");

        let image_path = r_task(|| unsafe {
            RFunction::from(".ps.graphics.renderPlotFromRecording")
                .param("id", id)
                .param("width", RObject::try_from(width)?)
                .param("height", RObject::try_from(height)?)
                .param("pixel_ratio", pixel_ratio)
                .param("format", format.to_string())
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

        let result = RFunction::from(".ps.graphics.recordPlot")
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
pub(crate) fn on_process_events() {
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

unsafe fn ps_graphics_device_impl() -> anyhow::Result<SEXP> {
    DEVICE_CONTEXT.with_borrow(|cell| {
        let p_device: *const libr::DevDescVersion16 = &cell.device;
        let p_device: *mut libr::DevDesc = p_device as *mut libr::DevDesc;
        let p_ge_device = libr::GEcreateDevDesc(p_device);
        let name = CString::new("Ark Graphics Device").unwrap();

        // Wrapper for:
        //
        // ```
        // gsetVar(R_DeviceSymbol, mkString(name), R_BaseEnv);
        // GEaddDevice(gdd);
        // GEinitDisplayList(gdd);
        // ```
        libr::GEaddDevice2(p_ge_device, name.as_ptr());
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

// ---------------------------------------------------------------------------------------
// Used hooks

fn new_device() -> libr::DevDescVersion16 {
    libr::DevDescVersion16 {
        // Screen dimensions in pts
        left: 0.0,
        right: 480.0,
        bottom: 480.0,
        top: 0.0,

        clipLeft: 0.0,
        clipRight: 0.0,
        clipBottom: 0.0,
        clipTop: 0.0,

        // Magic constants copied from other graphics devices
        xCharOffset: 0.4900,
        yCharOffset: 0.3333,
        yLineBias: 0.2,
        // Inches per raster
        ipr: [1.0 / 72.0, 1.0 / 72.0],
        // Character size in rasters
        cra: [0.9 * 12.0, 0.9 * 12.0],
        // (initial) device gamma correction
        gamma: 1.0,

        // Device capabilities
        canClip: libr::Rboolean_TRUE,
        canChangeGamma: libr::Rboolean_FALSE,
        // Can do at least some horiz adjust of text
        // 0 = none, 1 = {0,0.5,1}, 2 = [0,1]
        canHAdj: 2,

        // Device initial settings
        startps: 12.0,
        // Sets par("fg"), par("col"), and gpar("col")
        startcol: 0,
        // Sets par("bg") and gpar("fill")
        startfill: 0,
        startlty: 0,
        startfont: 1,
        startgamma: 1.0,

        // Device specific information
        deviceSpecific: std::ptr::null_mut(),

        // This one actually matters, as it enables display list recording to work
        displayListOn: 1,

        // Event handling entries
        // FALSE until we know any better
        canGenMouseDown: libr::Rboolean_FALSE,
        canGenMouseMove: libr::Rboolean_FALSE,
        canGenMouseUp: libr::Rboolean_FALSE,
        canGenKeybd: libr::Rboolean_FALSE,
        canGenIdle: libr::Rboolean_FALSE,

        // This is set while getGraphicsEvent is actively looking for events
        gettingEvent: libr::Rboolean_FALSE,

        // Device procedures
        activate: Some(hook_activate),
        circle: Some(hook_circle),
        clip: Some(hook_clip),
        close: Some(hook_close),
        deactivate: Some(hook_deactivate),
        // Paired with `haveLocator`
        locator: None,
        line: Some(hook_line),
        metricInfo: Some(hook_metric_info),
        mode: Some(hook_mode),
        newPage: Some(hook_new_page),
        polygon: Some(hook_polygon),
        polyline: Some(hook_polyline),
        rect: Some(hook_rect),
        path: Some(hook_path),
        // Paired with `haveRaster`
        raster: None,
        // Paired with `haveCapture`
        cap: None,
        size: Some(hook_size),
        strWidth: Some(hook_str_width),
        text: Some(hook_text),
        onExit: Some(hook_on_exit),
        getEvent: Some(hook_get_event),
        // Let R handle `par(ask = TRUE)` new frame confirmation
        newFrameConfirm: None,
        hasTextUTF8: libr::Rboolean_TRUE,
        textUTF8: Some(hook_text_utf8),
        strWidthUTF8: Some(hook_str_width_utf8),
        wantSymbolUTF8: libr::Rboolean_TRUE,
        useRotatedTextInContour: libr::Rboolean_TRUE,
        eventEnv: unsafe { R_NilValue },
        eventHelper: None,
        holdflush: Some(hook_holdflush),
        // 0 = unset, 1 = no, 2 = yes
        haveTransparency: 1,
        // 0 = unset, 1 = no, 2 = fully, 3 = semi
        haveTransparentBg: 1,
        haveRaster: 0,
        haveCapture: 0,
        haveLocator: 0,
        setPattern: Some(hook_set_pattern),
        releasePattern: Some(hook_release_pattern),
        setClipPath: Some(hook_set_clip_path),
        releaseClipPath: Some(hook_release_clip_path),
        setMask: Some(hook_set_mask),
        releaseMask: Some(hook_release_mask),
        // TODO: Oldest supported? Not sure what to put here.
        // Maybe we can fill it in per graphics device implementation.
        // It does effect if `defineGroup` and `useGroup` are even called.
        deviceVersion: 13,
        deviceClip: libr::Rboolean_FALSE,
        defineGroup: Some(hook_define_group),
        useGroup: Some(hook_use_group),
        releaseGroup: Some(hook_release_group),
        stroke: Some(hook_stroke),
        fill: Some(hook_fill),
        fillStroke: Some(hook_fill_stroke),
        capabilities: Some(hook_capabilities),
        glyph: Some(hook_glyph),

        reserved: [0; 64usize],
    }
}

/// Activation callback
///
/// Only used for logging
///
/// NOTE: May be called during [DeviceContext::render_plot], since this is done by
/// copying the graphics display list to a new plot device, and then closing that device.
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn hook_activate(_dev: pDevDesc) {
    log::trace!("Entering hook_activate");
}

/// Deactivation callback
///
/// NOTE: May be called during [DeviceContext::render_plot], since this is done by
/// copying the graphics display list to a new plot device, and then closing that device.
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn hook_deactivate(_dev: pDevDesc) {
    log::trace!("Entering hook_deactivate");

    DEVICE_CONTEXT.with_borrow(|cell| {
        cell.hook_deactivate();
    });
}

// mode = 0, graphics off
// mode = 1, graphics on
// mode = 2, graphical input on (ignored by most drivers)
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn hook_mode(mode: std::ffi::c_int, _dd: pDevDesc) {
    log::trace!("Entering hook_mode");

    DEVICE_CONTEXT.with_borrow(|cell| {
        cell.hook_mode(mode);
    });
}

#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn hook_new_page(_gc: pGEcontext, _dd: pDevDesc) {
    log::trace!("Entering hook_new_page");

    DEVICE_CONTEXT.with_borrow(|cell| {
        cell.hook_new_page();
    });
}

#[tracing::instrument(level = "trace", skip_all, fields(level = %level))]
unsafe extern "C-unwind" fn hook_holdflush(
    _dd: pDevDesc,
    level: std::ffi::c_int,
) -> std::ffi::c_int {
    log::trace!("Entering hook_holdflush");

    // Something is probably not right here
    DEVICE_CONTEXT.with_borrow(|cell| {
        cell.hook_holdflush(level);
        level
    })
}

// ---------------------------------------------------------------------------------------
// Empty hooks

unsafe extern "C-unwind" fn hook_circle(_x: f64, _y: f64, _r: f64, _gc: pGEcontext, _dd: pDevDesc) {
}

unsafe extern "C-unwind" fn hook_clip(_x0: f64, _x1: f64, _y0: f64, _y1: f64, _dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_close(_dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_line(
    _x1: f64,
    _y1: f64,
    _x2: f64,
    _y2: f64,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_metric_info(
    _c: std::ffi::c_int,
    _gc: pGEcontext,
    ascent: *mut f64,
    descent: *mut f64,
    width: *mut f64,
    _dd: pDevDesc,
) {
    // Copying {devoid}
    *ascent = 1.0;
    *descent = 1.0;
    *width = 1.0;
}

unsafe extern "C-unwind" fn hook_polygon(
    _n: std::ffi::c_int,
    _x: *mut f64,
    _y: *mut f64,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_polyline(
    _n: std::ffi::c_int,
    _x: *mut f64,
    _y: *mut f64,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_rect(
    _x0: f64,
    _y0: f64,
    _x1: f64,
    _y1: f64,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_path(
    _x: *mut f64,
    _y: *mut f64,
    _npoly: std::ffi::c_int,
    _nper: *mut std::ffi::c_int,
    _winding: libr::Rboolean,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_size(
    _left: *mut f64,
    _right: *mut f64,
    _bottom: *mut f64,
    _top: *mut f64,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_str_width(
    _str: *const std::ffi::c_char,
    _gc: pGEcontext,
    _dd: pDevDesc,
) -> f64 {
    0.0
}

unsafe extern "C-unwind" fn hook_text(
    _x: f64,
    _y: f64,
    _str: *const std::ffi::c_char,
    _rot: f64,
    _hadj: f64,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_on_exit(_dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_get_event(_arg1: SEXP, _arg2: *const std::ffi::c_char) -> SEXP {
    R_NilValue
}

unsafe extern "C-unwind" fn hook_text_utf8(
    _x: f64,
    _y: f64,
    _str: *const std::ffi::c_char,
    _rot: f64,
    _hadj: f64,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_str_width_utf8(
    _str: *const std::ffi::c_char,
    _gc: pGEcontext,
    _dd: pDevDesc,
) -> f64 {
    0.0
}

unsafe extern "C-unwind" fn hook_set_pattern(_pattern: SEXP, _dd: pDevDesc) -> SEXP {
    R_NilValue
}

unsafe extern "C-unwind" fn hook_release_pattern(_ref_: SEXP, _dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_set_clip_path(_path: SEXP, _ref_: SEXP, _dd: pDevDesc) -> SEXP {
    R_NilValue
}

unsafe extern "C-unwind" fn hook_release_clip_path(_ref_: SEXP, _dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_set_mask(_path: SEXP, _ref_: SEXP, _dd: pDevDesc) -> SEXP {
    R_NilValue
}

unsafe extern "C-unwind" fn hook_release_mask(_ref_: SEXP, _dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_define_group(
    _source: SEXP,
    _op: std::ffi::c_int,
    _destination: SEXP,
    _dd: pDevDesc,
) -> SEXP {
    R_NilValue
}

unsafe extern "C-unwind" fn hook_use_group(_ref_: SEXP, _trans: SEXP, _dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_release_group(_ref_: SEXP, _dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_stroke(_path: SEXP, _gc: pGEcontext, _dd: pDevDesc) {}

unsafe extern "C-unwind" fn hook_fill(
    _path: SEXP,
    _rule: std::ffi::c_int,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_fill_stroke(
    _path: SEXP,
    _rule: std::ffi::c_int,
    _gc: pGEcontext,
    _dd: pDevDesc,
) {
}

unsafe extern "C-unwind" fn hook_capabilities(_cap: SEXP) -> SEXP {
    R_NilValue
}

unsafe extern "C-unwind" fn hook_glyph(
    _n: std::ffi::c_int,
    _glyphs: *mut std::ffi::c_int,
    _x: *mut f64,
    _y: *mut f64,
    _font: SEXP,
    _size: f64,
    _colour: std::ffi::c_int,
    _rot: f64,
    _dd: pDevDesc,
) {
}
