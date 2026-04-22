//
// graphics_device.rs
//
// Copyright (C) 2022-2026 by Posit Software, PBC
//

// See `doc/graphics-devices.md` for documentation

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs::File;
use std::io::BufReader;
use std::io::Read;
use std::rc::Rc;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::plot_comm::IntrinsicSize;
use amalthea::comm::plot_comm::PlotBackendReply;
use amalthea::comm::plot_comm::PlotBackendRequest;
use amalthea::comm::plot_comm::PlotFrontendEvent;
use amalthea::comm::plot_comm::PlotMetadata;
use amalthea::comm::plot_comm::PlotOrigin;
use amalthea::comm::plot_comm::PlotRange;
use amalthea::comm::plot_comm::PlotRenderFormat;
use amalthea::comm::plot_comm::PlotRenderSettings;
use amalthea::comm::plot_comm::PlotResult;
use amalthea::comm::plot_comm::PlotSize;
use amalthea::comm::plot_comm::PlotUnit;
use amalthea::comm::plot_comm::UpdateParams;
use amalthea::socket::comm::CommOutgoingTx;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::display_data::DisplayData;
use amalthea::wire::execute_request::CodeLocation;
use amalthea::wire::execute_request::ExecuteRequestPositron;
use amalthea::wire::update_display_data::TransientValue;
use amalthea::wire::update_display_data::UpdateDisplayData;
use anyhow::anyhow;
use anyhow::Context;
use base64::prelude::*;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use libr::pDevDesc;
use libr::pGEcontext;
use libr::R_NilValue;
use libr::SEXP;
use serde_json::json;
use stdext::result::ResultExt;
use stdext::unwrap;
use uuid::Uuid;

use crate::comm_handler::handle_rpc_request;
use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::console::Console;
use crate::console::SessionMode;
use crate::modules::ARK_ENVS;
use crate::r_task;

pub const PLOT_COMM_NAME: &str = "positron.plot";

/// Perform R-side initialization of the graphics device.
/// Must be called from the main R thread after Console is initialized.
pub(crate) fn init_graphics_device() {
    // Declare our graphics device as interactive
    if let Err(err) = RFunction::from(".ps.graphics.register_as_interactive").call() {
        log::error!("Failed to register Ark graphics device as interactive: {err:?}");
    };
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
pub(crate) struct PlotId(String);

/// Execution context captured when an execute request starts.
/// Stored on the graphics device so it can be associated with plots created during execution.
#[derive(Clone, Default)]
struct ExecutionContext {
    execution_id: String,
    code: String,
    code_location: Option<CodeLocation>,
    /// Render settings override from the execute request (e.g. Quarto sizing metadata).
    /// When `Some`, used instead of `DeviceContext::prerender_settings` for pre-rendering.
    render_settings: Option<PlotRenderSettings>,
    /// Intrinsic size from the execute request (e.g. Quarto's fig-width/fig-height in inches).
    intrinsic_size: Option<IntrinsicSize>,
}

/// Per-plot context captured at creation time.
struct PlotContext {
    metadata: PlotMetadata,
    intrinsic_size: Option<IntrinsicSize>,
}

/// Graphics device state: plot recording, rendering, and comm management.
///
/// Fields use `Cell`/`RefCell` for interior mutability because the R graphics
/// device callbacks are C function pointers that receive `&DeviceContext` (via
/// `Console::get().device_context()`). There is no way to thread `&mut` through
/// R's callback registration layer. A future refactor could wrap the C-to-Rust
/// bridge so that the Rust-facing hook methods receive `&mut self` explicitly,
/// containing the `Console::get()` unsoundness in one place.
///
/// NOTE: Never hold a `RefCell` borrow while calling into R (`RObject::from`,
/// `RFunction::call`, `libr::Rf_*`, etc.). Any R call can in principle re-enter
/// Rust (e.g. via finalizers during GC), so keeping borrows short avoids
/// `RefCell` panics.
pub(crate) struct DeviceContext {
    /// Whether we are running in Console, Notebook, or Background mode.
    session_mode: SessionMode,

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

    /// Mapping of `PlotId` to comm ID, used for sending update events to
    /// existing plot comms via `CommOutgoingTx`.
    comm_ids: RefCell<HashMap<PlotId, String>>,

    /// Per-plot context captured at creation time (metadata and optional intrinsic size).
    plot_contexts: RefCell<HashMap<PlotId, PlotContext>>,

    /// Counters for generating unique plot names by kind
    kind_counters: RefCell<HashMap<String, u32>>,

    /// The callbacks of the wrapped device, initialized on graphics device creation
    wrapped_callbacks: WrappedDeviceCallbacks,

    /// The settings used for pre-renderings of new plots.
    prerender_settings: Cell<PlotRenderSettings>,

    /// The current execution context from the active request.
    /// Pushed here when an execute request starts via `on_execute_request()`,
    /// cleared when the request completes.
    execution_context: RefCell<Option<ExecutionContext>>,

    /// Stack of source file URIs, pushed/popped by the `source()` hook.
    /// When a plot is created inside `source("foo.R")`, the top of this stack
    /// provides the file attribution even though the execute_request came from the console.
    source_context_stack: RefCell<Vec<String>>,

    /// The plot origin captured eagerly when drawing starts (i.e. when `has_changes`
    /// transitions from false to true). This is necessary because the source context
    /// stack may be popped before `process_changes()` runs (e.g. `source()` completes
    /// before the execute request finishes), so we snapshot the origin at drawing time.
    pending_origin: RefCell<Option<Option<PlotOrigin>>>,
}

impl std::fmt::Debug for DeviceContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceContext").finish_non_exhaustive()
    }
}

impl DeviceContext {
    pub fn new(iopub_tx: Sender<IOPubMessage>, session_mode: SessionMode) -> Self {
        Self {
            session_mode,
            iopub_tx,
            has_changes: Cell::new(false),
            is_new_page: Cell::new(true),
            is_drawing: Cell::new(false),
            should_render: Cell::new(true),
            id: RefCell::new(Self::new_id()),
            comm_ids: RefCell::new(HashMap::new()),
            plot_contexts: RefCell::new(HashMap::new()),
            kind_counters: RefCell::new(HashMap::new()),
            wrapped_callbacks: WrappedDeviceCallbacks::default(),
            prerender_settings: Cell::new(PlotRenderSettings {
                size: PlotSize {
                    width: 640,
                    height: 400,
                },
                pixel_ratio: 1.,
                format: PlotRenderFormat::Png,
            }),
            execution_context: RefCell::new(None),
            source_context_stack: RefCell::new(Vec::new()),
            pending_origin: RefCell::new(None),
        }
    }

    pub fn set_prerender_settings(&self, settings: PlotRenderSettings) {
        self.prerender_settings.replace(settings);
    }

    /// Set the current execution context (called when an execute request starts)
    pub(crate) fn set_execution_context(
        &self,
        execution_id: String,
        code: String,
        code_location: Option<CodeLocation>,
        render_settings: Option<PlotRenderSettings>,
        intrinsic_size: Option<IntrinsicSize>,
    ) {
        *self.execution_context.borrow_mut() = Some(ExecutionContext {
            execution_id,
            code,
            code_location,
            render_settings,
            intrinsic_size,
        });
    }

    /// Clear the current execution context (called when an execute request completes)
    pub(crate) fn clear_execution_context(&self) {
        *self.execution_context.borrow_mut() = None;
    }

    /// Push a source file URI onto the stack (called when `source()` starts)
    fn push_source_context(&self, uri: String) {
        self.source_context_stack.borrow_mut().push(uri);
    }

    /// Pop a source file URI from the stack (called when `source()` completes)
    fn pop_source_context(&self) {
        self.source_context_stack.borrow_mut().pop();
    }

    /// Get the current source file URI, if inside a `source()` call
    fn current_source_uri(&self) -> Option<String> {
        self.source_context_stack.borrow().last().cloned()
    }

    /// Eagerly capture the plot origin so it's available when `process_changes()` runs later.
    /// Called when drawing first starts for a change set, since the source context stack
    /// may be popped before we get a chance to consume it.
    fn set_pending_origin(&self, origin: Option<PlotOrigin>) {
        self.pending_origin.replace(Some(origin));
    }

    /// Clear any unconsumed pending origin.
    pub(crate) fn clear_pending_origin(&self) {
        self.pending_origin.replace(None);
    }

    /// Create a new id for this new plot page (from Positron's perspective)
    /// and note that this is a new page
    fn new_positron_page(&self) {
        self.is_new_page.replace(true);
        self.id.replace(Self::new_id());
        self.clear_pending_origin();
    }

    /// Should plot events be sent over [CommSocket]s to the frontend?
    ///
    /// This allows plots to be dynamically resized by their `id`. Only possible if the UI
    /// comm is connected (i.e. we are connected to Positron) and if we are in
    /// [SessionMode::Console] mode.
    fn should_use_dynamic_plots(&self, console: &Console) -> bool {
        self.session_mode == SessionMode::Console && console.ui_comm().is_some()
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
    fn hook_deactivate(&self, console: &Console) {
        self.process_changes(console);
    }

    #[tracing::instrument(level = "trace", skip_all, fields(level = %level))]
    fn hook_holdflush(&self, level: i32, console: &Console) {
        // Be extra safe and check `level <= 0` rather than just `level == 0` in case
        // our shadowed device returns a negative `level`
        let is_released = level <= 0;
        let was_rendering = self.should_render.replace(is_released);

        // Flush deferred changes on hold→release transition
        if !was_rendering && is_released {
            self.process_changes(console);
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(mode = %mode))]
    fn hook_mode(&self, mode: i32) {
        let is_drawing = mode != 0;
        self.is_drawing.replace(is_drawing);
        let old_has_changes = self.has_changes.get();
        self.has_changes.replace(old_has_changes || is_drawing);

        // Eagerly capture the plot origin when drawing first starts for this
        // change set. The source context stack may be popped before
        // `process_changes()` runs, so we snapshot it now while it's available.
        if !old_has_changes && is_drawing {
            let ctx = self.capture_execution_context();
            let origin = self.capture_plot_origin(&ctx);
            self.set_pending_origin(origin);
        }
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

    /// Capture the current execution context for a new plot.
    ///
    /// Returns the context pushed via `graphics_on_execute_request()`, or an
    /// empty default for plots created outside of an execute request (e.g.
    /// during startup).
    fn capture_execution_context(&self) -> ExecutionContext {
        // No execution context was pushed. This can happen for plots created
        // outside of an execute request (e.g. during startup).
        self.execution_context.borrow().clone().unwrap_or_default()
    }

    /// Determine the plot origin for a new plot.
    ///
    /// Checks three sources in priority order:
    /// 1. Source context stack (inside `source("file.R")`) -- file-level, no range
    /// 2. Execute request's `code_location` (code run from a file via the IDE) -- with range
    /// 3. None (code typed at the console)
    ///
    /// When inside `source()`, the source file URI takes priority over the
    /// execute request's `code_location`, which would just point at the
    /// `source()` call itself.
    fn capture_plot_origin(&self, ctx: &ExecutionContext) -> Option<PlotOrigin> {
        // If we're inside a source() call, use the source file URI
        // (file-level attribution only, no line range)
        if let Some(source_uri) = self.current_source_uri() {
            return Some(PlotOrigin {
                uri: source_uri,
                range: None,
            });
        }

        // Otherwise, use the code_location from the execute request
        ctx.code_location
            .as_ref()
            .map(Self::code_location_to_origin)
    }

    /// Take the pending origin that was captured eagerly at drawing time.
    /// Falls back to capturing the origin now if none was pending.
    fn take_pending_origin(&self, ctx: &ExecutionContext) -> Option<PlotOrigin> {
        self.pending_origin
            .borrow_mut()
            .take()
            .unwrap_or_else(|| self.capture_plot_origin(ctx))
    }

    /// Detect the kind of plot from the recording.
    ///
    /// Calls into R to inspect the plot recording and/or `.Last.value`.
    fn detect_plot_kind(&self, id: &PlotId) -> String {
        let result = RFunction::from(".ps.graphics.detect_plot_kind")
            .param("id", id)
            .call();

        match result {
            Ok(kind) => kind.to::<String>().unwrap_or_else(|err| {
                log::warn!("Failed to convert plot kind to string: {err:?}");
                "plot".to_string()
            }),
            Err(err) => {
                log::warn!("Failed to detect plot kind: {err:?}");
                "plot".to_string()
            },
        }
    }

    /// Generate a unique name for a plot of the given kind
    fn generate_plot_name(&self, kind: &str) -> String {
        let mut counters = self.kind_counters.borrow_mut();
        let counter = counters.entry(kind.to_string()).or_insert(0);
        *counter += 1;
        format!("{} {}", kind, counter)
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
                let intrinsic_size = self
                    .plot_contexts
                    .borrow()
                    .get(id)
                    .and_then(|ctx| ctx.intrinsic_size.clone());
                Ok(PlotBackendReply::GetIntrinsicSizeReply(intrinsic_size))
            },
            PlotBackendRequest::GetMetadata => {
                log::trace!("PlotBackendRequest::GetMetadata");

                // Metadata was captured at plot creation time, just retrieve it
                let contexts = self.plot_contexts.borrow();
                let plot_metadata = match contexts.get(id) {
                    Some(ctx) => ctx.metadata.clone(),
                    None => {
                        // Fallback if metadata wasn't captured (shouldn't happen)
                        log::warn!("No metadata found for plot id {id}");
                        PlotMetadata {
                            name: "plot".to_string(),
                            kind: "plot".to_string(),
                            execution_id: String::new(),
                            code: String::new(),
                            origin: None,
                        }
                    },
                };

                Ok(PlotBackendReply::GetMetadataReply(plot_metadata))
            },
            PlotBackendRequest::Render(plot_meta) => {
                log::trace!("PlotBackendRequest::Render");

                let size = match plot_meta.size {
                    Some(size) => size,
                    None => {
                        // No explicit size requested — use intrinsic size if available
                        let intrinsic = self
                            .plot_contexts
                            .borrow()
                            .get(id)
                            .and_then(|ctx| ctx.intrinsic_size.clone());
                        match intrinsic {
                            Some(intrinsic) => intrinsic.to_plot_size(),
                            None => {
                                return Err(anyhow!(
                                    "No size provided for plot {id} and no intrinsic size available"
                                ));
                            },
                        }
                    },
                };

                let settings = PlotRenderSettings {
                    size: PlotSize {
                        width: size.width,
                        height: size.height,
                    },
                    pixel_ratio: plot_meta.pixel_ratio,
                    format: plot_meta.format,
                };

                let data = self.render_plot(id, &settings)?;
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
    fn on_plot_closed(&self, id: &PlotId) {
        self.comm_ids.borrow_mut().remove(id);
        self.plot_contexts.borrow_mut().remove(id);

        if let Err(err) = RFunction::from("remove_recording")
            .param("id", id)
            .call_in(ARK_ENVS.positron_ns)
        {
            log::error!("Can't clean up plot (id: {id}): {err:?}");
        }

        // If the currently active plot is closed, advance to a new Positron page
        // See https://github.com/posit-dev/positron/issues/6702.
        if self.id() == *id {
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

    /// Process outstanding plot changes
    ///
    /// Uses execution context stored via `on_execute_request()` or falls back to
    /// getting context from Console's active request.
    #[tracing::instrument(level = "trace", skip_all)]
    pub(crate) fn process_changes(&self, console: &Console) {
        let id = self.id();

        if !self.has_changes.get() {
            log::trace!("No changes to process for plot `id` {id}");
            return;
        }

        log::trace!("Processing changes for plot `id` {id}");

        // Always record the current display list, even when rendering is held.
        // `ps_graphics_before_plot_new` calls us to snapshot the display list
        // before a new page clears it. Skipping the recording here would
        // permanently lose the previous plot's state.
        //
        // Recording here overrides an existing recording for `id` if something
        // has changed between then and now, which is what we want, for example,
        // we want it when running this line by line:
        //
        // ```r
        // par(mfrow = c(2, 1))
        // plot(1) # Should get recorded with `id1`
        // plot(2) # Should record and overwrite `id1` because no new_page has been requested
        // ```
        Self::record_plot(&id);

        if !self.should_render.get() {
            // Keep `has_changes` set so we re-enter this branch after the hold
            // is released (via `hook_holdflush`) and send the notification then.
            log::trace!("Deferring notification for plot `id` {id} (rendering held)");
            return;
        }

        self.has_changes.replace(false);

        if self.is_new_page.replace(false) {
            self.process_new_plot(&id, console);
        } else {
            self.process_update_plot(&id, console);
        }
    }

    fn process_new_plot(&self, id: &PlotId, console: &Console) {
        if self.should_use_dynamic_plots(console) {
            self.process_new_plot_positron(id, console);
        } else {
            self.process_new_plot_jupyter_protocol(id);
        }
    }

    /// Convert a `CodeLocation` to a `PlotOrigin` for the plot metadata.
    fn code_location_to_origin(loc: &CodeLocation) -> PlotOrigin {
        PlotOrigin {
            uri: loc.uri.to_string(),
            range: Some(PlotRange {
                start_line: loc.start.line as i64,
                start_character: loc.start.character as i64,
                end_line: loc.end.line as i64,
                end_character: loc.end.character as i64,
            }),
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_new_plot_positron(&self, id: &PlotId, console: &Console) {
        log::trace!("Notifying Positron of new plot");

        let ctx = self.capture_execution_context();
        self.store_plot_context(id, &ctx);

        // Use render settings from the execute request if available, otherwise fall back
        // to the default prerender settings.
        let settings = ctx
            .render_settings
            .unwrap_or_else(|| self.prerender_settings.get());

        let open_data = match self.render_plot(id, &settings) {
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

        let plot_comm = PlotComm {
            id: id.clone(),
            open_data,
            device_context: console.device_context_rc(),
        };

        match console.comm_open_backend(PLOT_COMM_NAME, Box::new(plot_comm)) {
            Ok(comm_id) => {
                self.comm_ids.borrow_mut().insert(id.clone(), comm_id);
            },
            Err(err) => {
                log::error!("Failed to register plot comm: {err:?}");
            },
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_new_plot_jupyter_protocol(&self, id: &PlotId) {
        log::trace!("Notifying Jupyter frontend of new plot");

        let ctx = self.capture_execution_context();
        self.store_plot_context(id, &ctx);

        let data = unwrap!(self.create_display_data_plot(id, &ctx), Err(error) => {
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
        let Some(transient) = serde_json::to_value(transient).log_err() else {
            return;
        };

        log::info!("Sending display data to IOPub.");

        self.iopub_tx
            .send(IOPubMessage::DisplayData(DisplayData {
                data,
                metadata,
                transient,
            }))
            .log_err();
    }

    /// Store intrinsic size and metadata for a new plot from the execution context.
    fn store_plot_context(&self, id: &PlotId, ctx: &ExecutionContext) {
        let kind = self.detect_plot_kind(id);
        let name = self.generate_plot_name(&kind);
        let origin = self.take_pending_origin(ctx);

        self.plot_contexts
            .borrow_mut()
            .insert(id.clone(), PlotContext {
                metadata: PlotMetadata {
                    name,
                    kind,
                    execution_id: ctx.execution_id.clone(),
                    code: ctx.code.clone(),
                    origin,
                },
                intrinsic_size: ctx.intrinsic_size.clone(),
            });
    }

    fn process_update_plot(&self, id: &PlotId, console: &Console) {
        if self.should_use_dynamic_plots(console) {
            self.process_update_plot_positron(id);
        } else {
            self.process_update_plot_jupyter_protocol(id);
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_update_plot_positron(&self, id: &PlotId) {
        log::trace!("Notifying Positron of plot update");

        let comm_id = match self.comm_ids.borrow().get(id).cloned() {
            Some(id) => id,
            None => {
                log::error!("Can't find comm to update with id: {id}.");
                return;
            },
        };

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

        let Some(value) = serde_json::to_value(PlotFrontendEvent::Update(update_params)).log_err()
        else {
            return;
        };

        let outgoing_tx = CommOutgoingTx::new(comm_id, self.iopub_tx.clone());
        outgoing_tx
            .send(CommMsg::Data(value))
            .context(format!("Failed to send update message for id {id}."))
            .log_err();
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_update_plot_jupyter_protocol(&self, id: &PlotId) {
        log::trace!("Notifying Jupyter frontend of plot update");

        let ctx = self.capture_execution_context();
        let data = unwrap!(self.create_display_data_plot(id, &ctx), Err(error) => {
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
            .log_err();
    }

    fn create_display_data_plot(
        &self,
        id: &PlotId,
        ctx: &ExecutionContext,
    ) -> Result<serde_json::Value, anyhow::Error> {
        let base = ctx.render_settings.unwrap_or(PlotRenderSettings {
            size: PlotSize {
                width: 800,
                height: 600,
            },
            pixel_ratio: 1.0,
            format: PlotRenderFormat::Png,
        });

        let width = r_option_positive_f64("ark.plot.width")
            .map(|w| (w * DEFAULT_DPI).round() as i64)
            .unwrap_or(base.size.width);
        let height = r_option_positive_f64("ark.plot.height")
            .map(|h| (h * DEFAULT_DPI).round() as i64)
            .unwrap_or(base.size.height);
        let pixel_ratio = r_option_positive_f64("ark.plot.pixel_ratio").unwrap_or(base.pixel_ratio);

        let settings = PlotRenderSettings {
            size: PlotSize { width, height },
            pixel_ratio,
            format: base.format,
        };

        let data = unwrap!(self.render_plot(id, &settings), Err(error) => {
            return Err(anyhow!("Failed to render plot with id {id} due to: {error}."));
        });

        let mut map = serde_json::Map::new();
        map.insert("image/png".to_string(), serde_json::to_value(data)?);

        Ok(serde_json::Value::Object(map))
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn render_plot(&self, id: &PlotId, settings: &PlotRenderSettings) -> anyhow::Result<String> {
        log::trace!("Rendering plot");

        let image_path: String = RFunction::from(".ps.graphics.render_plot_from_recording")
            .param("id", id)
            .param("width", RObject::try_from(settings.size.width)?)
            .param("height", RObject::try_from(settings.size.height)?)
            .param("pixel_ratio", settings.pixel_ratio)
            .param("format", settings.format.to_string())
            .call()?
            .try_into()
            .map_err(|err: harp::Error| anyhow!("Failed to render plot with `id` {id}: {err:?}"))?;

        log::trace!("Rendered plot to {image_path}");

        let conn = File::open(image_path)?;
        let mut reader = BufReader::new(conn);

        let mut buffer = vec![];
        reader.read_to_end(&mut buffer)?;

        let data = BASE64_STANDARD_NO_PAD.encode(buffer);

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

/// Per-plot comm handler registered in Console's comm table.
/// Delegates RPC handling and lifecycle events to the shared `DeviceContext`.
#[derive(Debug)]
struct PlotComm {
    id: PlotId,
    open_data: serde_json::Value,
    device_context: Rc<DeviceContext>,
}

impl CommHandler for PlotComm {
    fn open_metadata(&self) -> serde_json::Value {
        self.open_data.clone()
    }

    fn handle_msg(&mut self, msg: CommMsg, ctx: &CommHandlerContext) {
        handle_rpc_request(&ctx.outgoing_tx, PLOT_COMM_NAME, msg, |req| {
            self.device_context.handle_rpc(req, &self.id)
        });
    }

    fn handle_close(&mut self, _ctx: &CommHandlerContext) {
        self.device_context.on_plot_closed(&self.id);
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

/// Default DPI for converting inches to pixels.
/// Matches R's default: 96 on macOS, 72 on Linux/Windows.
/// See `default_resolution_in_pixels_per_inch()` in graphics.R.
const DEFAULT_DPI: f64 = if cfg!(target_os = "macos") {
    96.0
} else {
    72.0
};

/// Default aspect ratio (width:height) used when only output_width_px is provided.
const DEFAULT_ASPECT_RATIO: f64 = 4.0 / 3.0;

trait IntrinsicSizeExt {
    /// Convert an intrinsic size to a logical-pixel-based `PlotSize`.
    ///
    /// Returns dimensions in CSS/logical pixels. The R rendering layer handles
    /// physical pixel scaling via the separate `pixel_ratio` parameter.
    fn to_plot_size(&self) -> PlotSize;
}

impl IntrinsicSizeExt for IntrinsicSize {
    fn to_plot_size(&self) -> PlotSize {
        match self.unit {
            PlotUnit::Inches => PlotSize {
                width: (self.width * DEFAULT_DPI).round() as i64,
                height: (self.height * DEFAULT_DPI).round() as i64,
            },
            PlotUnit::Pixels => PlotSize {
                width: self.width.round() as i64,
                height: self.height.round() as i64,
            },
        }
    }
}

trait FromExecuteRequest: Sized {
    fn from_execute_request(req: &ExecuteRequestPositron) -> Option<Self>;
}

impl FromExecuteRequest for PlotRenderSettings {
    /// Create render settings from an execute request's Positron metadata.
    ///
    /// If `fig_width`/`fig_height` are both set (Quarto), returns settings with
    /// size in logical pixels (inches * 96 DPI).
    ///
    /// Otherwise if `output_width_px` is set, returns settings at that width
    /// with a 4:3 aspect ratio.
    ///
    /// Sizes are in CSS/logical pixels. The R rendering layer handles physical
    /// pixel scaling via the separate `pixel_ratio` parameter.
    fn from_execute_request(req: &ExecuteRequestPositron) -> Option<Self> {
        let pixel_ratio = req.output_pixel_ratio.unwrap_or(1.0);

        if let (Some(w), Some(h)) = (req.fig_width, req.fig_height) {
            if w > 0.0 && h > 0.0 {
                return Some(Self {
                    size: PlotSize {
                        width: (w * DEFAULT_DPI).round() as i64,
                        height: (h * DEFAULT_DPI).round() as i64,
                    },
                    pixel_ratio,
                    format: PlotRenderFormat::Png,
                });
            }
        }

        if let Some(width_px) = req.output_width_px {
            if width_px > 0.0 {
                return Some(Self {
                    size: PlotSize {
                        width: width_px.round() as i64,
                        height: (width_px / DEFAULT_ASPECT_RATIO).round() as i64,
                    },
                    pixel_ratio,
                    format: PlotRenderFormat::Png,
                });
            }
        }

        None
    }
}

impl FromExecuteRequest for IntrinsicSize {
    /// Create an intrinsic size from an execute request's Positron metadata.
    ///
    /// Only returns `Some` when both `fig_width` and `fig_height` are set
    /// (i.e. Quarto sizing), providing the intrinsic size in inches.
    fn from_execute_request(req: &ExecuteRequestPositron) -> Option<Self> {
        if let (Some(w), Some(h)) = (req.fig_width, req.fig_height) {
            if w > 0.0 && h > 0.0 {
                return Some(Self {
                    width: w,
                    height: h,
                    unit: PlotUnit::Inches,
                    source: String::from("Quarto"),
                });
            }
        }

        None
    }
}

/// Compute render settings and intrinsic size from execute request metadata.
pub(crate) fn compute_plot_overrides(
    req: &ExecuteRequestPositron,
) -> (Option<PlotRenderSettings>, Option<IntrinsicSize>) {
    (
        PlotRenderSettings::from_execute_request(req),
        IntrinsicSize::from_execute_request(req),
    )
}

/// Run a closure with `&Console` and `&DeviceContext`, catching any panic at
/// the FFI boundary. Graphics device callbacks are invoked from R
///
/// Only used for logging
///
/// NOTE: May be called during [DeviceContext::render_plot], since this is done by
/// copying the graphics display list to a new plot device, and then closing that device.
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_activate(dev: pDevDesc) {
    log::trace!("Entering callback_activate");

    let dc = Console::get().device_context();
    if let Some(callback) = dc.wrapped_callbacks.activate.get() {
        callback(dev);
    }
}

/// Deactivation callback
///
/// NOTE: May be called during [DeviceContext::render_plot], since this is done by
/// copying the graphics display list to a new plot device, and then closing that device.
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_deactivate(dev: pDevDesc) {
    log::trace!("Entering callback_deactivate");

    let console = Console::get();
    let dc = console.device_context();

    // We run our hook first to record before we deactivate the underlying device,
    // in case device deactivation messes with the display list
    dc.hook_deactivate(console);
    if let Some(callback) = dc.wrapped_callbacks.deactivate.get() {
        callback(dev);
    }
}

#[tracing::instrument(level = "trace", skip_all, fields(level_delta = %level_delta))]
unsafe extern "C-unwind" fn callback_holdflush(dev: pDevDesc, level_delta: i32) -> i32 {
    log::trace!("Entering callback_holdflush");

    let console = Console::get();
    let dc = console.device_context();
    // If our wrapped device has a `holdflush()` method, we rely on it to apply
    // the `level_delta` (typically `+1` or `-1`) and return the new level. Otherwise
    // we follow the lead of `devholdflush()` in R and use a resolved `level` of `0`.
    // Notably, `grDevices::png()` with a Cairo backend does not have a holdflush
    // hook.
    // https://github.com/wch/r-source/blob/8cebcc0a5d99890839e5171f398da643d858dcca/src/library/grDevices/src/devices.c#L129-L138
    let level = match dc.wrapped_callbacks.holdflush.get() {
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
    dc.hook_holdflush(level, console);
    level
}

// mode = 0, graphics off
// mode = 1, graphics on
// mode = 2, graphical input on (ignored by most drivers)
#[tracing::instrument(level = "trace", skip_all, fields(mode = %mode))]
unsafe extern "C-unwind" fn callback_mode(mode: i32, dev: pDevDesc) {
    log::trace!("Entering callback_mode");

    let console = Console::get();
    let dc = console.device_context();
    if let Some(callback) = dc.wrapped_callbacks.mode.get() {
        callback(mode, dev);
    }
    dc.hook_mode(mode);
}

#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_new_page(dd: pGEcontext, dev: pDevDesc) {
    log::trace!("Entering callback_new_page");

    let dc = Console::get().device_context();
    if let Some(callback) = dc.wrapped_callbacks.newPage.get() {
        callback(dd, dev);
    }
    dc.hook_new_page();
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

        let wrapped_callbacks = &Console::get().device_context().wrapped_callbacks;

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
#[ark::register]
fn ps_graphics_before_plot_new(console: &Console, _name: SEXP) -> anyhow::Result<SEXP> {
    log::trace!("Entering ps_graphics_before_plot_new");

    // Process changes related to the last plot before opening a new page.
    // Particularly important if we make multiple plots in a single chunk.
    console.device_context().process_changes(console);

    Ok(harp::r_null())
}

/// Retrieve plot metadata by plot ID (display_id).
///
/// Returns a named list with fields: name, kind, execution_id, code, origin_uri.
/// Returns NULL if no metadata is found for the given ID.
#[tracing::instrument(level = "trace", skip_all)]
#[ark::register]
fn ps_graphics_get_metadata(console: &Console, id: SEXP) -> anyhow::Result<SEXP> {
    let id_str: String = RObject::view(id).try_into()?;
    let plot_id = PlotId(id_str);

    // Clone metadata out of the borrow before calling into R. R allocations
    // (`RObject::from()`, `Rf_setAttrib()`, etc.) can trigger finalizers or
    // error handlers that re-enter `plot_contexts.borrow_mut()`, which would
    // panic if the shared borrow were still held.
    let metadata = {
        let contexts = console.device_context().plot_contexts.borrow();
        contexts.get(&plot_id).map(|ctx| ctx.metadata.clone())
    };

    let Some(info) = metadata else {
        return Ok(harp::r_null());
    };

    let origin_uri = info.origin.as_ref().map(|o| o.uri.as_str()).unwrap_or("");

    let values: Vec<RObject> = vec![
        RObject::from(info.name.as_str()),
        RObject::from(info.kind.as_str()),
        RObject::from(info.execution_id.as_str()),
        RObject::from(info.code.as_str()),
        RObject::from(origin_uri),
    ];
    let list = RObject::try_from(values)?;

    let names: Vec<String> = vec![
        "name".to_string(),
        "kind".to_string(),
        "execution_id".to_string(),
        "code".to_string(),
        "origin_uri".to_string(),
    ];
    let names = RObject::from(names);
    libr::Rf_setAttrib(list.sexp, libr::R_NamesSymbol, names.sexp);

    Ok(list.sexp)
}

/// Return the current plot ID. Used by tests to verify that layout panels
/// share the same page (same ID) and that overflow creates a new page.
#[ark::register]
fn ps_graphics_current_plot_id(console: &Console) -> anyhow::Result<SEXP> {
    let id = console.device_context().id();
    Ok(RObject::from(&id).sexp)
}

/// Push a source file URI onto the source context stack.
/// Called from the `source()` hook when entering a sourced file.
#[ark::register]
fn ps_graphics_push_source_context(console: &Console, uri: SEXP) -> anyhow::Result<SEXP> {
    let uri_str: String = RObject::view(uri).try_into()?;
    console.device_context().push_source_context(uri_str);
    Ok(harp::r_null())
}

/// Pop a source file URI from the source context stack.
/// Called from the `source()` hook when leaving a sourced file.
#[ark::register]
fn ps_graphics_pop_source_context(console: &Console) -> anyhow::Result<SEXP> {
    console.device_context().pop_source_context();
    Ok(harp::r_null())
}

/// Returns the default DPI for the current OS.
/// Called from R to avoid duplicating OS-detection logic.
#[harp::register]
unsafe extern "C-unwind" fn ps_graphics_default_dpi() -> anyhow::Result<SEXP> {
    Ok(RObject::from(DEFAULT_DPI as i32).sexp)
}

/// Read a positive `f64` from an R option. Returns `None` if the option is
/// unset, not numeric, or not positive.
fn r_option_positive_f64(name: &str) -> Option<f64> {
    let value = r_task(|| {
        RFunction::from("getOption")
            .param("x", name)
            .call()?
            .to::<f64>()
    });
    match value {
        Ok(v) if v > 0.0 => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::SessionMode;

    fn test_device_context() -> DeviceContext {
        let (tx, _rx) = crossbeam::channel::unbounded();
        DeviceContext::new(tx, SessionMode::Console)
    }

    #[test]
    fn test_capture_execution_context_default_when_empty() {
        let dc = test_device_context();
        let ctx = dc.capture_execution_context();
        assert_eq!(ctx.execution_id, "");
        assert_eq!(ctx.code, "");
        assert!(ctx.code_location.is_none());
        assert!(ctx.render_settings.is_none());
        assert!(ctx.intrinsic_size.is_none());
    }

    #[test]
    fn test_capture_execution_context_returns_stored() {
        let dc = test_device_context();
        dc.set_execution_context(
            String::from("msg-123"),
            String::from("plot(1:10)"),
            None,
            None,
            None,
        );

        let ctx = dc.capture_execution_context();
        assert_eq!(ctx.execution_id, "msg-123");
        assert_eq!(ctx.code, "plot(1:10)");
    }

    #[test]
    fn test_capture_execution_context_after_clear() {
        let dc = test_device_context();
        dc.set_execution_context(
            String::from("msg-123"),
            String::from("plot(1:10)"),
            None,
            None,
            None,
        );
        dc.clear_execution_context();

        let ctx = dc.capture_execution_context();
        assert_eq!(ctx.execution_id, "");
        assert_eq!(ctx.code, "");
    }
}
