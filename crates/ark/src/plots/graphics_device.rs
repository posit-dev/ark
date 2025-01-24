//
// graphics_device.rs
//
// Copyright (C) 2022-2024 by Posit Software, PBC
//

use std::cell::Cell;
use std::cell::RefCell;
///
/// The Positron Graphics Device.
///
/// Rather than implement a separate graphics device, Positron
/// allows the user to select their own graphics device, and
/// then monkey-patches it in a way that allows us to hook into
/// the various graphics events.
///
/// This approach is similar in spirit to the RStudio approach,
/// but is vastly simpler as we no longer need to implement and
/// synchronize two separate graphics devices.
///
/// See also:
///
/// https://github.com/wch/r-source/blob/trunk/src/include/R_ext/GraphicsDevice.h
/// https://github.com/wch/r-source/blob/trunk/src/include/R_ext/GraphicsEngine.h
/// https://github.com/rstudio/rstudio/blob/main/src/cpp/r/session/graphics/RGraphicsDevice.cpp
///
use std::collections::HashMap;
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
use libr::Rf_ScalarLogical;
use libr::SEXP;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::unwrap;
use uuid::Uuid;

use crate::r_task;

thread_local! {
  // Safety: Set once by `RMain` on initialization
  static DEVICE_CONTEXT: RefCell<DeviceContext> = panic!("Must access `DEVICE_CONTEXT` from the R thread");
}

const POSITRON_PLOT_CHANNEL_ID: &str = "positron.plot";

// Expose thread initialization via function so we can keep the structs private
pub(crate) fn init_graphics_device() {
    DEVICE_CONTEXT.set(Default::default())
}

#[derive(Debug, Default)]
#[allow(non_snake_case)]
struct DeviceCallbacks {
    pub activate: Cell<Option<unsafe extern "C" fn(pDevDesc)>>,
    pub deactivate: Cell<Option<unsafe extern "C" fn(pDevDesc)>>,
    pub holdflush: Cell<Option<unsafe extern "C" fn(pDevDesc, i32) -> i32>>,
    pub mode: Cell<Option<unsafe extern "C" fn(i32, pDevDesc)>>,
    pub newPage: Cell<Option<unsafe extern "C" fn(pGEcontext, pDevDesc)>>,
}

#[derive(Default)]
struct DeviceContext {
    // Tracks whether the graphics device has changes.
    pub _changes: Cell<bool>,

    // Tracks whether or not the current plot page has ever been written to.
    pub _new_page: Cell<bool>,

    // Tracks the current graphics device mode.
    pub _mode: Cell<i32>,

    // The 'holdflush' flag, as normally handled via a device's 'holdflush()'
    // callback. If 'dev.hold()' has been set, we want to avoid rendering
    // new plots.
    pub _holdflush: Cell<i32>,

    // The ID associated with the current plot page. Used primarily
    // for accessing indexed plots, e.g. for the Plots pane history.
    pub _id: RefCell<Option<String>>,

    // A map, mapping plot IDs to the communication channels used
    // for communicating their rendered results to the frontend.
    pub _channels: RefCell<HashMap<String, CommSocket>>,

    // The device callbacks, which are patched into the device.
    pub _callbacks: DeviceCallbacks,
}

impl DeviceContext {
    pub fn holdflush(&self, holdflush: i32) {
        self._holdflush.replace(holdflush);
    }

    pub fn mode(&self, mode: i32, _dev: pDevDesc) {
        // Refcell safety: Only called on the R thread and we make sure not to
        // recurse into `DeviceContext` methods.
        self._mode.replace(mode);

        let old = self._changes.get();
        self._changes.replace(old || mode != 0);
    }

    pub fn new_page(&self, _dd: pGEcontext, _dev: pDevDesc) {
        // Create a new id for this new plot page and note that this is a new page
        let id = Uuid::new_v4().to_string();
        self._id.replace(Some(id));
        self._new_page.replace(true);
    }

    pub fn on_did_execute_request(
        &self,
        comm_manager_tx: Sender<CommManagerEvent>,
        iopub_tx: Sender<IOPubMessage>,
        dynamic_plots: bool,
    ) {
        // After R code has completed execution, we use this to check if any graphics
        // need to be created
        let changed = self._changes.replace(false);
        if changed {
            self.process_changes(comm_manager_tx, iopub_tx, dynamic_plots);
        }
    }

    pub fn on_process_events(&self) {
        // Don't try to render a plot if we're currently drawing.
        if self._mode.get() != 0 {
            return;
        }

        // Don't try to render a plot if the 'holdflush' flag is set.
        if self._holdflush.get() > 0 {
            return;
        }

        // Collect existing channels into a vector of tuples.
        // Necessary for handling Select in a clean way.
        let channels = {
            // Refcell Safety: Clone the hashmap so we don't hold a reference for too long
            let channels = self._channels.borrow().clone();
            channels.into_iter().collect::<Vec<_>>()
        };

        // Check for incoming plot render requests.
        let mut select = Select::new();
        for (_id, channel) in channels.iter() {
            select.recv(&channel.incoming_rx);
        }

        let selection = unwrap!(select.try_select(), Err(_error) => {
            // We don't log errors here, since it's most likely that none
            // of the channels have any messages available.
            return;
        });

        let plot_id = unsafe { &channels.get_unchecked(selection.index()).0 };
        let socket = unsafe { &channels.get_unchecked(selection.index()).1 };
        let message = unwrap!(selection.recv(&socket.incoming_rx), Err(error) => {
            log::error!("{}", error);
            return;
        });

        // Get the RPC request.
        if socket.handle_request(message, |req| self.handle_rpc(req, plot_id)) {
            return;
        }
    }

    fn handle_rpc(
        &self,
        message: PlotBackendRequest,
        plot_id: &String,
    ) -> anyhow::Result<PlotBackendReply> {
        match message {
            PlotBackendRequest::GetIntrinsicSize => {
                Ok(PlotBackendReply::GetIntrinsicSizeReply(None))
            },
            PlotBackendRequest::Render(plot_meta) => {
                let size = unwrap!(plot_meta.size, None => {
                    bail!("Intrinsically sized plots are not yet supported.");
                });
                let data = self.render_plot(
                    &plot_id,
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

    fn process_changes(
        &self,
        comm_manager_tx: Sender<CommManagerEvent>,
        iopub_tx: Sender<IOPubMessage>,
        dynamic_plots: bool,
    ) {
        // Refcell Safety: Short borrows in the file.
        let id = unwrap!(self._id.borrow().clone(), None => {
            log::error!("Unexpected uninitialized `id`.");
            return;
        });

        let new_page = self._new_page.replace(false);
        if new_page {
            self.process_new_plot(id.as_str(), comm_manager_tx, iopub_tx, dynamic_plots);
        } else {
            self.process_update_plot(id.as_str(), iopub_tx, dynamic_plots);
        }
    }

    fn process_new_plot(
        &self,
        id: &str,
        comm_manager_tx: Sender<CommManagerEvent>,
        iopub_tx: Sender<IOPubMessage>,
        dynamic_plots: bool,
    ) {
        if dynamic_plots {
            self.process_new_plot_positron(id, comm_manager_tx);
        } else {
            self.process_new_plot_jupyter_protocol(id, iopub_tx);
        }
    }

    fn process_new_plot_positron(&self, id: &str, comm_manager_tx: Sender<CommManagerEvent>) {
        // Let Positron know that we just created a new plot.
        let socket = CommSocket::new(
            CommInitiator::BackEnd,
            id.to_string(),
            POSITRON_PLOT_CHANNEL_ID.to_string(),
        );

        let event = CommManagerEvent::Opened(socket.clone(), serde_json::Value::Null);
        if let Err(error) = comm_manager_tx.send(event) {
            log::error!("{}", error);
        }

        // Save our new socket.
        // Refcell Safety: Short borrows in the file.
        self._channels
            .borrow_mut()
            .insert(id.to_string(), socket.clone());
    }

    fn process_new_plot_jupyter_protocol(&self, id: &str, iopub_tx: Sender<IOPubMessage>) {
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

        iopub_tx
            .send(IOPubMessage::DisplayData(DisplayData {
                data,
                metadata,
                transient,
            }))
            .or_log_warning(&format!("Could not publish display data on IOPub."));
    }

    fn process_update_plot(
        &self,
        id: &str,
        iopub_tx: Sender<IOPubMessage>,
        positron_connected: bool,
    ) {
        if positron_connected {
            self.process_update_plot_positron(id);
        } else {
            self.process_update_plot_jupyter_protocol(id, iopub_tx);
        }
    }

    fn process_update_plot_positron(&self, id: &str) {
        // Refcell Safety: Make sure not to call other methods from this whole block.
        let channels = self._channels.borrow();

        // Find our socket
        let socket = unwrap!(channels.get(id), None => {
            // If socket doesn't exist, bail, nothing to update (should be rare, likely a bug?)
            log::error!("Can't find socket to update with id: {id}.");
            return;
        });

        log::info!("Sending plot update message for id: {id}.");

        let value = serde_json::to_value(PlotFrontendEvent::Update).unwrap();

        // Tell Positron we have an updated plot that it should request a rerender for
        socket
            .outgoing_tx
            .send(CommMsg::Data(value))
            .or_log_error("Failed to send update message for id {id}.");
    }

    fn process_update_plot_jupyter_protocol(&self, id: &str, iopub_tx: Sender<IOPubMessage>) {
        let data = unwrap!(self.create_display_data_plot(id), Err(error) => {
            log::error!("Failed to create plot due to: {error}.");
            return;
        });

        let metadata = json!({});

        let transient = TransientValue {
            display_id: id.to_string(),
            data: None,
        };

        log::info!("Sending update display data to IOPub.");

        iopub_tx
            .send(IOPubMessage::UpdateDisplayData(UpdateDisplayData {
                data,
                metadata,
                transient,
            }))
            .or_log_warning(&format!("Could not publish update display data on IOPub."));
    }

    fn create_display_data_plot(&self, id: &str) -> Result<serde_json::Value, anyhow::Error> {
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

    fn render_plot(
        &self,
        plot_id: &str,
        width: i64,
        height: i64,
        pixel_ratio: f64,
        format: &RenderFormat,
    ) -> anyhow::Result<String> {
        // Render the plot to file.
        // TODO: Is it possible to do this without writing to file; e.g. could
        // we instead write to a connection or something else?
        let image_path = r_task(|| unsafe {
            RFunction::from(".ps.graphics.renderPlot")
                .param("id", plot_id)
                .param("width", RObject::try_from(width)?)
                .param("height", RObject::try_from(height)?)
                .param("dpr", pixel_ratio)
                .param("format", format.to_string())
                .call()?
                .to::<String>()
        });

        let image_path = unwrap!(image_path, Err(error) => {
            bail!("Failed to render plot with id {plot_id} due to: {error}.");
        });

        // Read contents into bytes.
        let conn = File::open(image_path)?;
        let mut reader = BufReader::new(conn);

        let mut buffer = vec![];
        reader.read_to_end(&mut buffer)?;

        // what an odd interface
        let data = general_purpose::STANDARD_NO_PAD.encode(buffer);

        Ok(data)
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

pub unsafe fn on_process_events() {
    DEVICE_CONTEXT.with_borrow(|cell| cell.on_process_events());
}

pub unsafe fn on_did_execute_request(
    comm_manager_tx: Sender<CommManagerEvent>,
    iopub_tx: Sender<IOPubMessage>,
    dynamic_plots: bool,
) {
    DEVICE_CONTEXT
        .with_borrow(|cell| cell.on_did_execute_request(comm_manager_tx, iopub_tx, dynamic_plots));
}

// NOTE: May be called when rendering a plot to file, since this is done by
// copying the graphics display list to a new plot device, and then closing that device.
unsafe extern "C" fn gd_activate(dev: pDevDesc) {
    log::trace!("gd_activate");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell._callbacks.activate.get() {
            callback(dev);
        }
    });
}

// NOTE: May be called when rendering a plot to file, since this is done by
// copying the graphics display list to a new plot device, and then closing that device.
unsafe extern "C" fn gd_deactivate(dev: pDevDesc) {
    log::trace!("gd_deactivate");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell._callbacks.deactivate.get() {
            callback(dev);
        }
    });
}

unsafe extern "C" fn gd_hold_flush(dev: pDevDesc, mut holdflush: i32) -> i32 {
    log::trace!("gd_hold_flush");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell._callbacks.holdflush.get() {
            holdflush = callback(dev, holdflush);
        }

        cell.holdflush(holdflush);
        holdflush
    })
}

// mode = 0, graphics off
// mode = 1, graphics on
// mode = 2, graphical input on (ignored by most drivers)
unsafe extern "C" fn gd_mode(mode: i32, dev: pDevDesc) {
    log::trace!("gd_mode: {mode}");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell._callbacks.mode.get() {
            callback(mode, dev);
        }
        cell.mode(mode, dev);
    });
}

unsafe extern "C" fn gd_new_page(dd: pGEcontext, dev: pDevDesc) {
    log::trace!("gd_new_page");

    DEVICE_CONTEXT.with_borrow(|cell| {
        if let Some(callback) = cell._callbacks.newPage.get() {
            callback(dd, dev);
        }
        cell.new_page(dd, dev);
    });
}

unsafe fn ps_graphics_device_impl() -> anyhow::Result<SEXP> {
    // TODO: Don't allow creation of more than one graphics device.
    // TODO: Allow customization of the graphics device here?

    // TODO: Infer appropriate resolution based on whether display is high DPI.
    let res = 144;

    // TODO: allow customization of device type.
    let r#type = RObject::null();

    // Create the graphics device.
    RFunction::from(".ps.graphics.createDevice")
        .param("name", "Positron Graphics Device")
        .param("type", r#type)
        .param("res", res)
        .call()?;

    // Get reference to current device (opaque pointer)
    let ge_device = libr::GEcurrentDevice();

    // Initialize display list (needed for copying of plots)
    // (Called on opaque pointer, because that matches the function signature.
    // Pointer specialization is done below, at which point we can access and set
    // `displayListOn` too)
    libr::GEinitDisplayList(ge_device);

    // Get a specialized versioned pointer from our opaque one so we can initialize our _callbacks
    with_device!(ge_device, |ge_device, device| {
        (*ge_device).displayListOn = 1;
        // (*ge_device).recordGraphics = 1;

        DEVICE_CONTEXT.with_borrow(|cell| {
            let callbacks = &cell._callbacks;

            // Safety: The callbacks are stored in simple cells.

            callbacks.activate.replace((*device).activate);
            (*device).activate = Some(gd_activate);

            callbacks.deactivate.replace((*device).deactivate);
            (*device).deactivate = Some(gd_deactivate);

            callbacks.holdflush.replace((*device).holdflush);
            (*device).holdflush = Some(gd_hold_flush);

            callbacks.mode.replace((*device).mode);
            (*device).mode = Some(gd_mode);

            callbacks.newPage.replace((*device).newPage);
            (*device).newPage = Some(gd_new_page);
        });
    });

    Ok(R_NilValue)
}

#[harp::register]
unsafe extern "C" fn ps_graphics_device() -> anyhow::Result<SEXP> {
    ps_graphics_device_impl().or_else(|err| {
        log::error!("{}", err);
        Ok(R_NilValue)
    })
}

#[harp::register]
unsafe extern "C" fn ps_graphics_event(_name: SEXP) -> anyhow::Result<SEXP> {
    let id = unwrap!(DEVICE_CONTEXT.with_borrow(|cell| cell._id.borrow().clone()), None => {
        return Ok(Rf_ScalarLogical(0));
    });

    let result = RFunction::from(".ps.graphics.createSnapshot")
        .param("id", id)
        .call();

    if let Err(error) = result {
        log::error!("{}", error);
        return Ok(Rf_ScalarLogical(0));
    }

    Ok(Rf_ScalarLogical(1))
}
