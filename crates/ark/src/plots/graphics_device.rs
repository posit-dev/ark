//
// graphics_device.rs
//
// Copyright (C) 2022-2024 by Posit Software, PBC
//

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
use std::ops::Deref;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;
use std::sync::RwLock;

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

const POSITRON_PLOT_CHANNEL_ID: &str = "positron.plot";

thread_local! {
    static DEVICE_CONTEXT: DeviceContext = DeviceContext::default();
}

macro_rules! trace {
    ($($tts:tt)*) => {{
        let message = format!($($tts)*);
        log::info!("[graphics] {}", message);
    }}
}

#[derive(Debug, Default)]
#[allow(non_snake_case)]
struct DeviceCallbacks {
    pub activate: Option<unsafe extern "C" fn(pDevDesc)>,
    pub deactivate: Option<unsafe extern "C" fn(pDevDesc)>,
    pub holdflush: Option<unsafe extern "C" fn(pDevDesc, i32) -> i32>,
    pub mode: Option<unsafe extern "C" fn(i32, pDevDesc)>,
    pub newPage: Option<unsafe extern "C" fn(pGEcontext, pDevDesc)>,
}

#[derive(Default)]
struct DeviceContext {
    // Tracks whether the graphics device has changes.
    pub _changes: AtomicBool,

    // Tracks whether or not the current plot page has ever been written to.
    pub _new_page: AtomicBool,

    // Tracks the current graphics device mode.
    pub _mode: AtomicI32,

    // The 'holdflush' flag, as normally handled via a device's 'holdflush()'
    // callback. If 'dev.hold()' has been set, we want to avoid rendering
    // new plots.
    pub _holdflush: AtomicI32,

    // Whether we're currently rendering a plot. Mainly used to avoid
    // recursive plot invocations.
    pub _rendering: AtomicBool,

    // The ID associated with the current plot page. Used primarily
    // for accessing indexed plots, e.g. for the Plots pane history.
    pub _id: RwLock<Option<String>>,

    // A map, mapping plot IDs to the communication channels used
    // for communicating their rendered results to the frontend.
    pub _channels: RwLock<HashMap<String, CommSocket>>,

    // The device callbacks, which are patched into the device.
    pub _callbacks: RwLock<DeviceCallbacks>,
}

impl DeviceContext {
    pub fn holdflush(&self, holdflush: i32) {
        self._holdflush.store(holdflush, Ordering::Relaxed);
    }

    pub fn mode(&self, mode: i32, _dev: pDevDesc) {
        self._mode.store(mode, Ordering::Relaxed);
        self._changes.fetch_or(mode != 0, Ordering::Relaxed);
    }

    pub fn new_page(&self, _dd: pGEcontext, _dev: pDevDesc) {
        // Create a new id for this new plot page and note that this is a new page
        self._id
            .write()
            .map(|mut id| *id = Some(Uuid::new_v4().to_string()))
            .expect("Can write to `_id`");
        self._new_page.store(true, Ordering::Relaxed);
    }

    pub fn on_did_execute_request(
        &self,
        comm_manager_tx: Sender<CommManagerEvent>,
        iopub_tx: Sender<IOPubMessage>,
        dynamic_plots: bool,
    ) {
        // After R code has completed execution, we use this to check if any graphics
        // need to be created
        if self._changes.swap(false, Ordering::Relaxed) {
            self.process_changes(comm_manager_tx, iopub_tx, dynamic_plots);
        }
    }

    pub fn on_process_events(&self) {
        // Don't try to render a plot if we're currently drawing.
        if self._mode.load(Ordering::Relaxed) != 0 {
            return;
        }

        // Don't try to render a plot if the 'holdflush' flag is set.
        if self._holdflush.load(Ordering::Relaxed) > 0 {
            return;
        }

        // Check for incoming plot render requests.
        let (plot_id, socket, message) = match self._channels.read() {
            Ok(channels) => {
                // Must have a vector for order stability in `selection.index()`
                let channels = channels.iter().collect::<Vec<_>>();

                let mut select = Select::new();
                for (_id, channel) in channels.iter() {
                    select.recv(&channel.incoming_rx);
                }

                let selection = unwrap!(select.try_select(), Err(_error) => {
                    // We don't log errors here, since it's most likely that none
                    // of the channels have any messages available.
                    return;
                });

                let channel = channels.get(selection.index()).unwrap();

                let message = unwrap!(selection.recv(&channel.1.incoming_rx), Err(error) => {
                    log::error!("{}", error);
                    return;
                });

                // We are careful to clone the `channel` here, to avoid holding the
                // read lock on `_channels` across the R call in `handle_rpc()`'s call
                // to `render_plot()`, which in theory could loop back into here
                (channel.0.clone(), channel.1.clone(), message)
            },
            Err(error) => {
                log::error!("{error}");
                return;
            },
        };

        // Get the RPC request.
        if socket.handle_request(message, |req| Self::handle_rpc(req, &plot_id)) {
            return;
        }
    }

    fn handle_rpc(
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
                let data = Self::render_plot(
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
        let id = match self._id.read() {
            Ok(id) => match id.deref() {
                Some(id) => id.clone(),
                None => {
                    log::error!("Unexpected uninitialized `id`.");
                    return;
                },
            },
            Err(error) => {
                log::error!("{error}");
                return;
            },
        };

        if self._new_page.swap(false, Ordering::Relaxed) {
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
        match self._channels.write() {
            Ok(mut channels) => {
                channels.insert(id.to_string(), socket.clone());
            },
            Err(error) => {
                log::error!("{error}")
            },
        }
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
        // Find our socket
        match self._channels.read() {
            Ok(channels) => match channels.get(id) {
                Some(socket) => {
                    log::info!("Sending plot update message for id: {id}.");
                    let value = serde_json::to_value(PlotFrontendEvent::Update).unwrap();

                    // Tell Positron we have an updated plot that it should request a rerender for
                    socket
                        .outgoing_tx
                        .send(CommMsg::Data(value))
                        .or_log_error("Failed to send update message for id {id}.");
                },
                None => {
                    // If socket doesn't exist, bail, nothing to update (should be rare, likely a bug?)
                    log::error!("Can't find socket to update with id: {id}.");
                    return;
                },
            },
            Err(error) => {
                log::error!("{error}");
                return;
            },
        }
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

        let data = unwrap!(Self::render_plot(id, width, height, pixel_ratio, &format), Err(error) => {
            bail!("Failed to render plot with id {id} due to: {error}.");
        });

        let mut map = serde_json::Map::new();
        map.insert("image/png".to_string(), serde_json::to_value(data).unwrap());

        Ok(serde_json::Value::Object(map))
    }

    fn render_plot(
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
    DEVICE_CONTEXT.with(|context| context.on_process_events());
}

pub unsafe fn on_did_execute_request(
    comm_manager_tx: Sender<CommManagerEvent>,
    iopub_tx: Sender<IOPubMessage>,
    dynamic_plots: bool,
) {
    DEVICE_CONTEXT
        .with(|context| context.on_did_execute_request(comm_manager_tx, iopub_tx, dynamic_plots));
}

// NOTE: May be called when rendering a plot to file, since this is done by
// copying the graphics display list to a new plot device, and then closing that device.
unsafe extern "C" fn gd_activate(dev: pDevDesc) {
    trace!("gd_activate");

    DEVICE_CONTEXT.with(|context| {
        let callbacks = context._callbacks.read().unwrap();
        callbacks.activate.map(|callback| callback(dev))
    });
}

// NOTE: May be called when rendering a plot to file, since this is done by
// copying the graphics display list to a new plot device, and then closing that device.
unsafe extern "C" fn gd_deactivate(dev: pDevDesc) {
    trace!("gd_deactivate");

    DEVICE_CONTEXT.with(|context| {
        let callbacks = context._callbacks.read().unwrap();
        callbacks.deactivate.map(|callback| callback(dev))
    });
}

unsafe extern "C" fn gd_hold_flush(dev: pDevDesc, holdflush: i32) -> i32 {
    trace!("gd_hold_flush");

    DEVICE_CONTEXT.with(|context| {
        let callbacks = context._callbacks.read().unwrap();
        callbacks.holdflush.map(|callback| callback(dev, holdflush));
        context.holdflush(holdflush);
    });

    holdflush
}

// mode = 0, graphics off
// mode = 1, graphics on
// mode = 2, graphical input on (ignored by most drivers)
unsafe extern "C" fn gd_mode(mode: i32, dev: pDevDesc) {
    trace!("gd_mode: {}", mode);

    DEVICE_CONTEXT.with(|context| {
        // Call regular callback
        let callbacks = context._callbacks.read().unwrap();
        callbacks.mode.map(|callback| callback(mode, dev));

        // Call our handler
        context.mode(mode, dev);
    });
}

unsafe extern "C" fn gd_new_page(dd: pGEcontext, dev: pDevDesc) {
    trace!("gd_new_page");

    DEVICE_CONTEXT.with(|context| {
        // Call regular callback
        let callbacks = context._callbacks.read().unwrap();
        callbacks.newPage.map(|callback| callback(dd, dev));

        // Call our handler
        context.new_page(dd, dev);
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

        // device description struct
        DEVICE_CONTEXT.with(|context| {
            let mut callbacks = context._callbacks.write().unwrap();

            callbacks.activate = (*device).activate;
            (*device).activate = Some(gd_activate);

            callbacks.deactivate = (*device).deactivate;
            (*device).deactivate = Some(gd_deactivate);

            callbacks.holdflush = (*device).holdflush;
            (*device).holdflush = Some(gd_hold_flush);

            callbacks.mode = (*device).mode;
            (*device).mode = Some(gd_mode);

            callbacks.newPage = (*device).newPage;
            (*device).newPage = Some(gd_new_page);
        })
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
    let id = DEVICE_CONTEXT.with(|context| {
        let id = context._id.read().unwrap();
        id.clone()
    });

    let id = match id {
        Some(id) => id,
        None => {
            return Ok(Rf_ScalarLogical(0));
        },
    };

    let result = RFunction::from(".ps.graphics.createSnapshot")
        .param("id", id)
        .call();

    if let Err(error) = result {
        log::error!("{}", error);
        return Ok(Rf_ScalarLogical(0));
    }

    Ok(Rf_ScalarLogical(1))
}
