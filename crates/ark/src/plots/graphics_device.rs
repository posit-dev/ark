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
use std::thread;
use std::time::Duration;
use std::time::Instant;

use amalthea::comm::base_comm::JsonRpcErrorCode;
use amalthea::comm::base_comm::RpcError;
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
use amalthea::comm::ui_comm::LocatePlotPointParams;
use amalthea::comm::ui_comm::PlotPoint;
use amalthea::comm::ui_comm::UiFrontendReply;
use amalthea::comm::ui_comm::UiFrontendRequest;
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
use harp::exec::r_sandbox;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::raii::RLocalInteractive;
use harp::raii::RLocalInterruptsSuspended;
use libr::pDevDesc;
use libr::pGEcontext;
use libr::R_NilValue;
use libr::Rboolean;
use libr::Rboolean_FALSE;
use libr::Rboolean_TRUE;
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
use crate::plots::device_state::DeviceState;
use crate::plots::device_state::Notification;
use crate::r_task;
use crate::signals::interrupts_pending;
use crate::signals::set_interrupts_pending;

pub const PLOT_COMM_NAME: &str = "positron.plot";

/// Perform R-side initialization of the graphics device.
/// Must be called from the main R thread after Console is initialized.
pub(crate) fn init_graphics_device(session_mode: SessionMode) {
    // In notebooks, don't declare our graphics device as interactive. If it
    // were, `dev.interactive()` would be `TRUE` and `plot.lm()` and similar
    // functions would prompt "Hit <Return> to see next plot" between pages. A
    // notebook frontend has no good way to answer, so the user would see only
    // the last page (posit-dev/positron#14534). Jupyter kernels such as
    // IRkernel also use a non-interactive device.
    if session_mode == SessionMode::Notebook {
        return;
    }

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
    close: Cell<Option<unsafe extern "C-unwind" fn(pDevDesc)>>,
    deactivate: Cell<Option<unsafe extern "C-unwind" fn(pDevDesc)>>,
    holdflush: Cell<Option<unsafe extern "C-unwind" fn(pDevDesc, i32) -> i32>>,
    mode: Cell<Option<unsafe extern "C-unwind" fn(i32, pDevDesc)>>,
    newPage: Cell<Option<unsafe extern "C-unwind" fn(pGEcontext, pDevDesc)>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub(crate) struct PlotId(String);

impl PlotId {
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

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
    /// Channel for sending [IOPubMessage::DisplayData] and
    /// [IOPubMessage::UpdateDisplayData] to Jupyter frontends when plot events occur
    iopub_tx: Sender<IOPubMessage>,

    /// The current page and the `dev.hold()` state. Together they decide
    /// whether a plot change is sent to the frontend at once, or after
    /// `dev.flush()` releases a hold.
    state: RefCell<DeviceState>,

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

    /// The Ark graphics device. Set when the device is created, cleared when
    /// it is closed.
    device: Cell<Option<libr::pGEDevDesc>>,

    /// The last Plots pane size the frontend sent in a console session, in CSS
    /// pixels. The Ark device's bounds follow it. `None` until the frontend
    /// sends a size (always `None` in notebook sessions), in which case the
    /// device keeps the size it was created with.
    pane_size: Cell<Option<PlotSize>>,

    /// The plot ID and render size of the last pre-render sent to the frontend.
    /// Lets a caller skip rendering the same plot at the same size again.
    last_prerender: RefCell<Option<(PlotId, PlotSize)>>,

    /// The length of the Ark device's display list when `locate()` last
    /// returned a point, if no new page started since. R records a
    /// `locator()` call in the display list only when it returns, so while the
    /// length is unchanged, the points `locator(type =)` draws between clicks
    /// are not in a recording yet. See [DeviceContext::locate].
    located_display_list_length: Cell<Option<isize>>,

    /// The last mode R set with the `mode` hook. See [DeviceContext::hook_mode].
    mode: Cell<i32>,

    /// Whether we are replaying the display list after a Plots pane resize
    /// (see [DeviceContext::set_pane_size]).
    is_replaying: Cell<bool>,

    /// The current execution context from the active request. Pushed here when
    /// an execute request starts via `graphics_on_execute_request()`, cleared
    /// when the request completes.
    execution_context: RefCell<Option<ExecutionContext>>,

    /// Stack of source file URIs, pushed/popped by the `source()` hook.
    /// When a plot is created inside `source("foo.R")`, the top of this stack
    /// provides the file attribution even though the execute_request came from the console.
    source_context_stack: RefCell<Vec<String>>,

    /// Snapshot the origin at draw time because `source()` can pop its context
    /// before `process_changes()` runs. The snapshot is tagged with `PlotId` so a
    /// stale origin can't be attributed to a later page.
    pending_origin: RefCell<Option<(PlotId, Option<PlotOrigin>)>>,
}

impl std::fmt::Debug for DeviceContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceContext").finish_non_exhaustive()
    }
}

impl DeviceContext {
    pub fn new(iopub_tx: Sender<IOPubMessage>) -> Self {
        Self {
            iopub_tx,
            state: RefCell::new(DeviceState::new()),
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
            device: Cell::new(None),
            pane_size: Cell::new(None),
            last_prerender: RefCell::new(None),
            located_display_list_length: Cell::new(None),
            mode: Cell::new(0),
            is_replaying: Cell::new(false),
            execution_context: RefCell::new(None),
            source_context_stack: RefCell::new(Vec::new()),
            pending_origin: RefCell::new(None),
        }
    }

    pub fn set_prerender_settings(&self, settings: PlotRenderSettings) {
        self.prerender_settings.replace(settings);
    }

    /// Make the Ark device follow the Plots pane size, so `dev.size()`,
    /// `par("din")`, and grid report the size the plot is shown at.
    ///
    /// Only call this in console sessions: notebook plots are not sized by the
    /// Plots pane. If the device doesn't exist yet, the size is applied when
    /// it is created. If the bounds change at the top-level prompt, the display
    /// list is replayed without plot events.
    pub(crate) fn set_pane_size(&self, size: PlotSize) {
        self.pane_size.set(Some(size));
        if !self.resize_device(size) {
            return;
        }

        // Replay only at the top-level prompt. At a nested prompt (`browser()`,
        // a breakpoint, `readline()`), the code under the prompt may still be
        // drawing, so the display list can be incomplete. The next
        // `plot.new()` uses the new bounds.
        match harp::session::r_n_frame() {
            Ok(0) => {},
            Ok(_) => return,
            Err(err) => {
                log::warn!("Can't determine if we're at top level, not replaying: {err:?}");
                return;
            },
        }

        // Replay the display list so base graphics recomputes its state (such
        // as `par("din")`) for the new bounds. The replay runs R code, so it
        // runs in a sandbox and errors are only logged: the user didn't ask
        // for it. Not interactive, so `readline()` in replayed code returns at
        // once (`r_read_console()` handles `browser()`). Created outside of
        // `r_sandbox()` so that its `Drop` runs in a frame that an R longjump
        // never crosses.
        let _interactive = RLocalInteractive::new(false);
        self.is_replaying.set(true);
        let result =
            r_sandbox(|| RFunction::from(".ps.graphics.replay_quietly").call()).and_then(|r| r);
        self.is_replaying.set(false);
        if let Err(err) = result {
            log::warn!("Can't replay the display list after a resize: {err:?}");
        }
    }

    /// Whether we are replaying the display list after a Plots pane resize.
    /// While this is `true`, the plot hooks do nothing (the replay is not a
    /// plot change) and the console returns EOF at nested prompts.
    pub(crate) fn is_replaying(&self) -> bool {
        self.is_replaying.get()
    }

    /// Set the Ark device's bounds to `size` (in CSS pixels). Returns `true`
    /// if the bounds changed. Does nothing and returns `false` if there is no
    /// Ark device. Never calls into R.
    fn resize_device(&self, size: PlotSize) -> bool {
        let Some(ge_device) = self.device.get() else {
            return false;
        };
        // Safety: `device` is cleared in `callback_close()`, so it points to a
        // live device.
        unsafe { set_device_bounds(ge_device, size) }
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
        // Between requests the frontend may have rendered plots again at
        // other sizes, so the last pre-render may no longer be what it shows
        self.last_prerender.replace(None);

        self.located_display_list_length.replace(None);

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

    fn set_pending_origin(&self, origin: Option<PlotOrigin>) {
        self.pending_origin.replace(Some((self.id(), origin)));
    }

    fn has_pending_origin(&self) -> bool {
        match self.pending_origin.borrow().as_ref() {
            Some((page, _)) => *page == self.id(),
            None => false,
        }
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
    fn hook_deactivate(self: &Rc<Self>, console: &Console) {
        // `GEplayDisplayList()` selects the Ark device and then the previous
        // device, so this runs on every replay
        if self.is_replaying() {
            return;
        }
        self.process_changes(console);
    }

    #[tracing::instrument(level = "trace", skip_all, fields(level = %level))]
    fn hook_holdflush(self: &Rc<Self>, level: i32, console: &Console) {
        if self.is_replaying() {
            return;
        }

        let held_pages = self.state.borrow_mut().on_hold_level(level);
        let Some(held_pages) = held_pages else {
            return;
        };

        // The held pages are already out of the state, so the nested
        // `process_changes()` calls that rendering causes (it replays a
        // recording on another device, which deactivates ours) can't send
        // them twice. A new page's display or comm open can reach the frontend
        // before an earlier page's update. Frontends match updates by display
        // ID or comm ID, so this order is fine.
        self.process_changes(console);

        for id in held_pages {
            log::trace!("Processing held changes for plot `id` {id}");
            self.process_update_plot(&id, console);
        }
    }

    #[tracing::instrument(level = "trace", skip_all, fields(mode = %mode))]
    fn hook_mode(&self, mode: i32) {
        // A replay draws the existing plot again, it doesn't change it
        if self.is_replaying() {
            return;
        }

        // `mode == 1` means we are drawing, `mode == 0` means we stop drawing
        let is_drawing = mode == 1;

        // `identify()` draws its labels in mode 2 (graphical input), which
        // isn't drawing mode. After each label it switches to mode 0 and back
        // to 2, and it does that only when it drew a label. So a switch back
        // to 2 between the clicks of one call means the call drew something.
        let previous_mode = self.mode.replace(mode);
        let drew_during_input = mode == 2 && previous_mode == 0 && self.is_between_clicks();

        if !is_drawing && !drew_during_input {
            return;
        }
        self.state.borrow_mut().on_draw();

        // Capture each page's origin before `source()` pops its context
        if is_drawing && !self.has_pending_origin() {
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
        // A replay starts the existing page again, it isn't a new plot
        if self.is_replaying() {
            return;
        }

        self.state.borrow_mut().on_new_page();
        self.located_display_list_length.replace(None);
    }

    fn id(&self) -> PlotId {
        // Refcell Safety: Short borrows in the file.
        self.state.borrow().id().clone()
    }

    /// Capture the current execution context for a new plot.
    ///
    /// Returns the context pushed via `graphics_on_execute_request()`, or an
    /// empty default for plots created outside of an execute request (e.g.
    /// during startup).
    fn capture_execution_context(&self) -> ExecutionContext {
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

    /// Take the origin captured at drawing time for this page. Falls back to
    /// capturing the origin now if the snapshot is missing or belongs to an
    /// earlier page.
    fn take_pending_origin(&self, id: &PlotId, ctx: &ExecutionContext) -> Option<PlotOrigin> {
        let pending = self.pending_origin.borrow_mut().take();

        match pending {
            Some((page, origin)) if page == *id => origin,
            _ => self.capture_plot_origin(ctx),
        }
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

                // The frontend may show this render instead of our last
                // pre-render, so `locate()` must not trust that one anymore
                self.last_prerender.replace(None);

                let mut pixel_ratio = plot_meta.pixel_ratio;
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
                            Some(intrinsic) => {
                                // Lift the resolution of physical (inch-based)
                                // sizes so they stay crisp when scaled to fit.
                                pixel_ratio = pixel_ratio.max(intrinsic.min_pixel_ratio());
                                intrinsic.to_plot_size()
                            },
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
                    pixel_ratio,
                    format: plot_meta.format,
                };

                let data = match self.render_plot(id, &settings) {
                    Ok(data) => data,
                    Err(err) if Self::is_too_small_error(&err) => {
                        return Err(RpcError {
                            code: JsonRpcErrorCode::PlotTooSmall,
                            message: format!("Plot {id} is too small to render: {err}"),
                        }
                        .into());
                    },
                    Err(err) => return Err(err),
                };
                let mime_type = Self::get_mime_type(&plot_meta.format);

                Ok(PlotBackendReply::RenderReply(PlotResult {
                    data,
                    mime_type: mime_type.to_string(),
                    settings: Some(settings),
                }))
            },
        }
    }

    #[tracing::instrument(level = "trace", skip(self))]
    fn on_plot_closed(&self, id: &PlotId) {
        self.state.borrow_mut().on_plot_closed(id);
        self.comm_ids.borrow_mut().remove(id);
        self.plot_contexts.borrow_mut().remove(id);

        if let Err(err) = RFunction::from("remove_recording")
            .param("id", id)
            .call_in(ARK_ENVS.positron_ns)
        {
            log::error!("Can't clean up plot (id: {id}): {err:?}");
        }
    }

    fn get_mime_type(format: &PlotRenderFormat) -> &'static str {
        match format {
            PlotRenderFormat::Png => "image/png",
            PlotRenderFormat::Svg => "image/svg+xml",
            PlotRenderFormat::Pdf => "application/pdf",
            PlotRenderFormat::Jpeg => "image/jpeg",
            PlotRenderFormat::Tiff => "image/tiff",
        }
    }

    /// Process outstanding plot changes
    ///
    /// Uses execution context stored via `graphics_on_execute_request()`.
    #[tracing::instrument(level = "trace", skip_all)]
    pub(crate) fn process_changes(self: &Rc<Self>, console: &Console) {
        let changed_page = self.state.borrow().changed_page().cloned();
        let Some(id) = changed_page else {
            log::trace!("No changes to process for plot `id` {}", self.id());
            return;
        };

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
        let recorded = Self::record_plot(&id);

        // While held, the page keeps its changes, so `hook_holdflush()` sends
        // it at release
        let notification = self.state.borrow_mut().on_recorded(&id, recorded);
        let Some(notification) = notification else {
            log::trace!("Deferring notification for plot `id` {id} (rendering held)");
            return;
        };

        self.send_notification(notification, console);
    }

    /// Process outstanding plot changes before R waits for user input, for
    /// example from `readline()`, `menu()`, or the `devAskNewPage()` prompt.
    ///
    /// When the device isn't held, this is [DeviceContext::process_changes].
    /// When it is held, a page is sent only if it had changes drawn before the
    /// hold, so `dev.hold()` keeps hiding pages that were drawn entirely while
    /// held (posit-dev/positron#7680).
    ///
    /// The page is sent from its latest recording. In the `devAskNewPage()`
    /// case, `before.plot.new` has already recorded the finished page and
    /// nothing has been drawn since the hold. If there is no recording since
    /// the hold, we record now, so drawing done while held on a page that was
    /// partly drawn before the hold can appear at the prompt instead of at
    /// `dev.flush()`.
    #[tracing::instrument(level = "trace", skip_all)]
    pub(crate) fn process_changes_before_input(self: &Rc<Self>, console: &Console) {
        if !self.state.borrow().is_held() {
            self.process_changes(console);
            return;
        }

        let changes = self.state.borrow_mut().take_pre_hold_changes();
        let Some(changes) = changes else {
            return;
        };

        log::trace!(
            "Sending pre-hold changes for plot `id` {} before input request",
            changes.id
        );

        let recorded = !changes.needs_recording() || Self::record_plot(&changes.id);

        let sent = self
            .state
            .borrow_mut()
            .notify_pre_hold_changes(changes, recorded);
        self.send_notification(sent.notification, console);
        self.state
            .borrow_mut()
            .on_pre_hold_changes_sent(sent.redrawn);
    }

    fn send_notification(self: &Rc<Self>, notification: Notification, console: &Console) {
        match notification {
            Notification::New(id) => self.process_new_plot(&id, console),
            Notification::Update(id) => self.process_update_plot(&id, console),
        }
    }

    /// Ask the frontend for the point the user clicks on the current plot, for
    /// `locator()` and friends. See "Locator" in `doc/graphics-devices.md`.
    ///
    /// Before the request, the frontend gets the current page rendered at the
    /// device size, so the fractions in the reply map to the device.
    #[tracing::instrument(level = "trace", skip_all)]
    pub(crate) fn locate(self: &Rc<Self>, console: &Console) -> anyhow::Result<LocateOutcome> {
        if !self.should_use_dynamic_plots(console) {
            return Ok(LocateOutcome::Finished);
        }

        // Replayed code that asks for a point, for example in `recordGraphics()`,
        // must not wait for the user: the replay runs while the frontend waits
        // for a resize to finish. Finish at once, as `readline()` does.
        if self.is_replaying() {
            log::trace!("Replaying the display list, not asking for a point");
            return Ok(LocateOutcome::Finished);
        }

        // R renders the page in a top-level context. An interrupt during the
        // render would end the render there and clear the interrupt, and the
        // stdin thread has no request to cancel yet. So suspend interrupts
        // while we prepare the page. An interrupt stays pending and ends
        // `locator()` here.
        let prepared = {
            let _interrupts = RLocalInterruptsSuspended::new(true);
            self.prepare_locate(console)
        };
        if interrupts_pending() {
            return Ok(LocateOutcome::Interrupted);
        }
        let Some((comm_id, size)) = prepared else {
            return Ok(LocateOutcome::Finished);
        };

        let Some(ui_comm) = console.ui_comm() else {
            return Ok(LocateOutcome::Finished);
        };
        // The frontend takes clicks only on an image of this size, so the
        // point maps to the device even if an older image is still shown
        let request = UiFrontendRequest::LocatePlotPoint(LocatePlotPointParams {
            plot_id: comm_id,
            width: size.width,
            height: size.height,
        });

        let outcome = match ui_comm.call_frontend_method_typed(request) {
            Ok(Some(UiFrontendReply::LocatePlotPointReply(Some(point)))) => {
                Ok(LocateOutcome::Point(point))
            },
            Ok(Some(UiFrontendReply::LocatePlotPointReply(None))) => Ok(LocateOutcome::Finished),
            Ok(Some(reply)) => Err(anyhow!(
                "Unexpected reply to `locate_plot_point`: {reply:?}"
            )),
            Ok(None) => Ok(LocateOutcome::Interrupted),
            Err(err) => Err(err),
        };

        // Only a point continues the `locator()` call
        let length = match outcome {
            Ok(LocateOutcome::Point(_)) => self.display_list_length(),
            _ => None,
        };
        self.located_display_list_length.replace(length);

        outcome
    }

    /// Sends the page that `locate()` asks for a point on: the current page,
    /// then a pre-render at the device size. Returns the plot comm ID and the
    /// size of the image, or `None` if there is no image to click on.
    fn prepare_locate(self: &Rc<Self>, console: &Console) -> Option<(String, PlotSize)> {
        // On the second and later clicks of one `locator()` call, the only
        // changes are the points or lines `locator(type =)` drew. They are not
        // in the display list until `locator()` returns, so sending the page
        // now would only render it again without them. Leave the page's
        // changes in place so it is sent with them after `locator()` returns.
        // The display list grows when `locator()` returns, so an unchanged
        // length means we are still in the same call.
        if !self.is_between_clicks() {
            self.state.borrow_mut().on_locate();

            // A `locator()` call in the same request as the plot runs before
            // the end-of-request flush, so send the page now
            self.process_changes_before_input(console);
        }

        let id = self.id();
        let Some(comm_id) = self.comm_ids.borrow().get(&id).cloned() else {
            // The frontend doesn't have this page, for example because
            // `dev.hold()` hides it, so there is nothing to click on
            log::trace!("No plot comm for plot `id` {id}, can't locate");
            return None;
        };

        let prerender = self.prerender_settings.get();
        let size = self.device_size().unwrap_or(prerender.size);

        // `last_prerender` is cleared when an execute request starts, so a
        // match means the frontend shows a pre-render of this page from this
        // request at the device size. Pages are rendered again whenever they
        // change, so it is current.
        let is_sent = matches!(
            &*self.last_prerender.borrow(),
            Some((last_id, last_size)) if *last_id == id && *last_size == size
        );
        if !is_sent {
            // Without a hold, the flush above recorded any changes, and only
            // drawing between clicks is left to record. While held, keep
            // sending the latest recording so we don't reveal drawing that the
            // hold hides. The page keeps its changes either way, so it is sent
            // again after `locator()` returns.
            let needs_recording = {
                let state = self.state.borrow();
                !state.is_held() && state.changed_page().is_some()
            };
            if needs_recording {
                Self::record_plot(&id);
            }

            // The frontend takes clicks only on an image of this size, and it
            // can't ask for one while R waits here. Without the image the user
            // could only finish, so finish now.
            if !self.send_plot_update(&id, PlotRenderSettings { size, ..prerender }) {
                log::warn!("Can't send the plot image for plot `id` {id}, can't locate");
                return None;
            }
        }

        Some((comm_id, size))
    }

    /// Whether R is between the clicks of one `locator()`, `identify()`, or
    /// `grid::grid.locator()` call: `locate()` returned a point, and the
    /// display list hasn't grown since. R records the call in the display
    /// list when it returns.
    fn is_between_clicks(&self) -> bool {
        self.located_display_list_length
            .get()
            .is_some_and(|length| Some(length) == self.display_list_length())
    }

    fn process_new_plot(self: &Rc<Self>, id: &PlotId, console: &Console) {
        if self.should_use_dynamic_plots(console) {
            self.process_new_plot_positron(id, console);
        } else {
            self.process_new_plot_jupyter_protocol(id);
        }
    }

    /// Dynamic plots require both Console mode and a connected UI comm so the
    /// frontend can issue resize/render RPCs against the per-plot comm.
    fn should_use_dynamic_plots(&self, console: &Console) -> bool {
        console.session_mode() == SessionMode::Console && console.ui_comm().is_some()
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
    fn process_new_plot_positron(self: &Rc<Self>, id: &PlotId, console: &Console) {
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
                self.last_prerender
                    .replace(Some((id.clone(), settings.size)));

                let pre_render = PlotResult {
                    data: pre_render,
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
            device_context: Rc::clone(self),
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

        let (data, metadata) = unwrap!(self.create_display_data_plot(id, &ctx), Err(error) => {
            log::error!("Failed to create plot due to: {error}.");
            return;
        });

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

        log::trace!("Sending display data to IOPub.");

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
        let origin = self.take_pending_origin(id, ctx);

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
        self.send_plot_update(id, self.prerender_settings.get());
    }

    /// Pre-render plot `id` with `settings` and send it to the frontend as a
    /// plot update event. Returns `true` if the update carries the
    /// pre-render. If the render fails, the update has none, and the frontend
    /// asks for a render itself.
    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn send_plot_update(&self, id: &PlotId, settings: PlotRenderSettings) -> bool {
        log::trace!("Notifying Positron of plot update");

        let comm_id = match self.comm_ids.borrow().get(id).cloned() {
            Some(id) => id,
            None => {
                log::error!("Can't find comm to update with id: {id}.");
                return false;
            },
        };

        let mut has_pre_render = false;
        let update_params = match self.render_plot(id, &settings) {
            Ok(pre_render) => {
                has_pre_render = true;
                let mime_type = Self::get_mime_type(&settings.format);
                self.last_prerender
                    .replace(Some((id.clone(), settings.size)));

                let pre_render = PlotResult {
                    data: pre_render,
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
            return false;
        };

        let outgoing_tx = CommOutgoingTx::new(comm_id, self.iopub_tx.clone());
        outgoing_tx
            .send(CommMsg::Data(value))
            .context(format!("Failed to send update message for id {id}."))
            .log_err()
            .is_some() &&
            has_pre_render
    }

    #[tracing::instrument(level = "trace", skip_all, fields(id = %id))]
    fn process_update_plot_jupyter_protocol(&self, id: &PlotId) {
        log::trace!("Notifying Jupyter frontend of plot update");

        let ctx = self.capture_execution_context();
        let (data, metadata) = unwrap!(self.create_display_data_plot(id, &ctx), Err(error) => {
            log::error!("Failed to create plot due to: {error}.");
            return;
        });

        let transient = TransientValue {
            display_id: id.to_string(),
            data: None,
        };

        log::trace!("Sending update display data to IOPub for `id` {id}.");

        self.iopub_tx
            .send(IOPubMessage::UpdateDisplayData(UpdateDisplayData {
                data,
                metadata,
                transient,
            }))
            .log_err();
    }

    /// Renders a plot for Jupyter and returns its display data and metadata.
    ///
    /// The image metadata contains logical dimensions so frontends display HiDPI
    /// renders at their intended size rather than at their physical pixel dimensions.
    fn create_display_data_plot(
        &self,
        id: &PlotId,
        ctx: &ExecutionContext,
    ) -> Result<(serde_json::Value, serde_json::Value), anyhow::Error> {
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

        let mime_type = Self::get_mime_type(&settings.format);

        let data = json!({ mime_type: data });

        let metadata = json!({
            mime_type: {
                "width": settings.size.width,
                "height": settings.size.height,
            }
        });

        Ok((data, metadata))
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

    /// Whether a render failed because the plot doesn't fit the requested size.
    fn is_too_small_error(err: &anyhow::Error) -> bool {
        let Some(harp::Error::TryCatchError(err)) = err.downcast_ref::<harp::Error>() else {
            return false;
        };

        let is_too_small = RFunction::from(".ps.graphics.is_too_small_message")
            .param("message", err.message.as_str())
            .call()
            .and_then(|result| result.to::<bool>());

        match is_too_small {
            Ok(is_too_small) => is_too_small,
            Err(err) => {
                log::warn!("Failed to classify plot render error: {err:?}");
                false
            },
        }
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

/// The result of [DeviceContext::locate]
#[derive(Debug)]
pub(crate) enum LocateOutcome {
    /// The user clicked this point
    Point(PlotPoint),
    /// The user finished without a click, or there is nothing to click on
    Finished,
    /// An interrupt cancelled the request
    Interrupted,
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

/// Set the bounds of `ge_device` to `size` (in CSS pixels). Returns `true` if
/// the bounds changed.
///
/// Converts through inches with the device's own `ipr`, so this is correct for
/// any device units. A pane of `w` CSS pixels is `w / DEFAULT_DPI` inches,
/// the same size the plot is rendered at (see `finalize_device_arguments()`).
/// Keeps the device's y direction (`bottom > top` for devices whose y axis
/// points down).
///
/// Only the bounds change. The wrapped device keeps its internal canvas, which
/// is fine because nothing reads the shadow device's pixels: we render plots on
/// a separate device.
unsafe fn set_device_bounds(ge_device: libr::pGEDevDesc, size: PlotSize) -> bool {
    let width = size.width as f64 / DEFAULT_DPI;
    let height = size.height as f64 / DEFAULT_DPI;

    let changed;
    with_device!(ge_device, |_ge_device, device| {
        let right = (*device).left + width / (*device).ipr[0];
        let delta_y = height / (*device).ipr[1];
        let (bottom, top) = if (*device).bottom >= (*device).top {
            ((*device).top + delta_y, (*device).top)
        } else {
            ((*device).bottom, (*device).bottom + delta_y)
        };

        changed = right != (*device).right || bottom != (*device).bottom || top != (*device).top;

        (*device).right = right;
        (*device).bottom = bottom;
        (*device).top = top;
    });
    changed
}

// Defined after `with_device!()`, which it uses.
impl DeviceContext {
    /// The length of the Ark device's display list, or `None` if there is no
    /// Ark device. Doesn't allocate or longjump.
    fn display_list_length(&self) -> Option<isize> {
        let ge_device = self.device.get()?;
        let length;
        // Safety: `device` is cleared in `callback_close()`, so it points to a
        // live device. `Rf_xlength()` only walks the pairlist.
        unsafe {
            with_device!(ge_device, |ge_device, _device| {
                length = libr::Rf_xlength((*ge_device).displayList) as isize;
            });
        }
        Some(length)
    }

    /// The Ark device's size in CSS pixels, or `None` if there is no Ark
    /// device. The inverse of `set_device_bounds()`. Never calls into R.
    pub(crate) fn device_size(&self) -> Option<PlotSize> {
        let ge_device = self.device.get()?;
        let size;
        // Safety: `device` is cleared in `callback_close()`, so it points to a
        // live device.
        unsafe {
            with_device!(ge_device, |_ge_device, device| {
                let width = ((*device).right - (*device).left).abs() * (*device).ipr[0];
                let height = ((*device).bottom - (*device).top).abs() * (*device).ipr[1];
                size = Some(PlotSize {
                    width: (width * DEFAULT_DPI).round() as i64,
                    height: (height * DEFAULT_DPI).round() as i64,
                });
            });
        }
        size
    }
}

/// Whether `ge_device` wraps the device `dev`
unsafe fn is_device(ge_device: libr::pGEDevDesc, dev: pDevDesc) -> bool {
    let is_device;
    with_device!(ge_device, |_ge_device, device| {
        is_device = device as pDevDesc == dev;
    });
    is_device
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
/// Matches the logical screen DPI at 100% display scaling on all platforms, and
/// matches RStudio. High-DPI scaling is applied separately via `pixel_ratio`.
/// See `default_resolution_in_pixels_per_inch()` in graphics.R.
const DEFAULT_DPI: f64 = 96.0;

/// Default aspect ratio (width:height) used when only output_width_px is provided.
const DEFAULT_ASPECT_RATIO: f64 = 4.0 / 3.0;

/// Target DPI for rendering inch-based intrinsic sizes (e.g. Quarto figures).
/// Quarto's HTML default renders figures at `fig-dpi` (96) scaled by
/// `fig.retina` (2), i.e. 192 DPI. Matching it keeps intrinsic plots crisp when
/// the plot pane scales them to fit, independent of the OS base screen DPI.
const INTRINSIC_RENDER_DPI: f64 = 192.0;

/// Default figure size in inches, matching Quarto's base/HTML format
/// defaults (`fig-width: 7`, `fig-height: 5`). Other formats differ (e.g.
/// pdf is 5.5 x 3.5), but Positron doesn't know the target format.
const DEFAULT_FIG_WIDTH: f64 = 7.0;
const DEFAULT_FIG_HEIGHT: f64 = 5.0;

trait IntrinsicSizeExt {
    /// Convert an intrinsic size to a logical-pixel-based `PlotSize`.
    ///
    /// Returns dimensions in CSS/logical pixels. The R rendering layer handles
    /// physical pixel scaling via the separate `pixel_ratio` parameter.
    fn to_plot_size(&self) -> PlotSize;

    /// Minimum pixel ratio for rendering this intrinsic size crisply.
    ///
    /// An inch-based size (Quarto) is a physical size that the plot pane may
    /// scale up to fit, so it needs enough resolution to lift the effective DPI
    /// (`DEFAULT_DPI * pixel_ratio`) up to `INTRINSIC_RENDER_DPI`. A pixel-based
    /// size already carries its own resolution, so no floor is applied.
    fn min_pixel_ratio(&self) -> f64;
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

    fn min_pixel_ratio(&self) -> f64 {
        match self.unit {
            PlotUnit::Inches => INTRINSIC_RENDER_DPI / DEFAULT_DPI,
            PlotUnit::Pixels => 1.0,
        }
    }
}

trait FromExecuteRequest: Sized {
    fn from_execute_request(req: &ExecuteRequestPositron) -> Option<Self>;
}

impl FromExecuteRequest for PlotRenderSettings {
    /// Create render settings from an execute request's Positron metadata.
    ///
    /// If `fig_width` and/or `fig_height` is set (Quarto), returns settings
    /// with size in logical pixels (inches * DPI). Either dimension may be set
    /// alone; the missing one falls back to the Quarto default.
    ///
    /// Otherwise if `output_width_px` is set, returns settings at that width
    /// with a 4:3 aspect ratio.
    ///
    /// Sizes are in CSS/logical pixels. The R rendering layer handles physical
    /// pixel scaling via the separate `pixel_ratio` parameter.
    fn from_execute_request(req: &ExecuteRequestPositron) -> Option<Self> {
        let pixel_ratio = req.output_pixel_ratio.unwrap_or(1.0);

        if let Some((width, height)) = requested_fig_size(req) {
            return Some(Self {
                size: PlotSize {
                    width: (width * DEFAULT_DPI).round() as i64,
                    height: (height * DEFAULT_DPI).round() as i64,
                },
                pixel_ratio,
                format: PlotRenderFormat::Png,
            });
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
    /// Only returns `Some` when `fig_width` and/or `fig_height` is set
    /// (i.e. Quarto sizing), providing the intrinsic size in inches.
    fn from_execute_request(req: &ExecuteRequestPositron) -> Option<Self> {
        let (width, height) = requested_fig_size(req)?;

        Some(Self {
            width,
            height,
            unit: PlotUnit::Inches,
            source: String::from("Quarto"),
        })
    }
}

/// The figure size requested via `fig-width`/`fig-height`, in inches.
///
/// Either option may be set alone, matching `quarto render`; the missing (or
/// non-positive) dimension falls back to the Quarto default. Returns `None`
/// when neither dimension is set.
fn requested_fig_size(req: &ExecuteRequestPositron) -> Option<(f64, f64)> {
    let width = req.fig_width.filter(|width| *width > 0.0);
    let height = req.fig_height.filter(|height| *height > 0.0);

    if width.is_none() && height.is_none() {
        return None;
    }

    Some((
        width.unwrap_or(DEFAULT_FIG_WIDTH),
        height.unwrap_or(DEFAULT_FIG_HEIGHT),
    ))
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

/// Activation callback
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
    let dc = Rc::clone(console.device_context());

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
    let dc = Rc::clone(console.device_context());
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
// mode = 2, graphical input on (ignored by most drivers). It is not a plot
// change, so `hook_mode()` treats only mode 1 as drawing.
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

#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_close(dev: pDevDesc) {
    log::trace!("Entering callback_close");

    let dc = Console::get().device_context();

    // R's `removeDevice()` calls `close` before `GEdestroyDevDesc()`, so the
    // stored device is still valid here
    if let Some(ge_device) = dc.device.get() {
        if is_device(ge_device, dev) {
            dc.device.set(None);
        }
    }

    if let Some(callback) = dc.wrapped_callbacks.close.get() {
        callback(dev);
    }
}

/// Size callback
///
/// Reports the bounds of `dev`, which follow the Plots pane (see
/// [DeviceContext::set_pane_size]). `dev.size()` and grid read the device size
/// through this callback. We never call the wrapped device's `size`, because it
/// reports the fixed size the device was created with.
#[tracing::instrument(level = "trace", skip_all)]
unsafe extern "C-unwind" fn callback_size(
    left: *mut f64,
    right: *mut f64,
    bottom: *mut f64,
    top: *mut f64,
    dev: pDevDesc,
) {
    // The bounds are the first fields of every `DevDesc` version
    let bounds = dev as *mut libr::DevDescBounds;
    *left = (*bounds).left;
    *right = (*bounds).right;
    *bottom = (*bounds).bottom;
    *top = (*bounds).top;
}

/// Locator callback
///
/// Used by `locator()`, `identify()`, and `grid::grid.locator()`. Asks the
/// frontend for a click on the current plot (see [DeviceContext::locate]).
/// Returns `FALSE` when the user finishes, which ends `locator()` with the
/// points so far.
unsafe extern "C-unwind" fn callback_locator(x: *mut f64, y: *mut f64, dev: pDevDesc) -> Rboolean {
    log::trace!("Entering callback_locator");

    match locate_device_point(dev) {
        DevicePoint::Point(point_x, point_y) => {
            *x = point_x;
            *y = point_y;
            Rboolean_TRUE
        },
        DevicePoint::Finished => Rboolean_FALSE,
        DevicePoint::Interrupted => {
            // `locator()` returns normally after `FALSE`, so without this the
            // rest of the input would run. Raise the interrupt here. Amalthea
            // cancels the frontend request before it signals R, so wait for
            // the interrupt to be pending. If we marked it pending ourselves,
            // the signal could arrive after we raised it and interrupt the
            // code that runs next.
            //
            // This longjumps out of the callback, which is why
            // `locate_device_point()` returns plain values: no destructor is
            // skipped, and no `Console` reference is alive while
            // `R_ProcessEvents()` borrows the console mutably.
            wait_for_pending_interrupt();
            libr::R_CheckUserInterrupt();
            Rboolean_FALSE
        },
    }
}

/// Waits for the interrupt that cancelled a frontend request to reach R. The
/// interrupt handler marks it pending right after Amalthea cancels the
/// request. If that doesn't happen within a second, marks it pending itself,
/// so the interrupt is never lost.
fn wait_for_pending_interrupt() {
    let deadline = Instant::now() + Duration::from_secs(1);
    while !interrupts_pending() {
        if Instant::now() >= deadline {
            log::warn!("Interrupt not pending after a locator request was interrupted");
            set_interrupts_pending(true);
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

/// A point from [DeviceContext::locate] in device coordinates. Holds no values
/// with destructors, see [callback_locator].
enum DevicePoint {
    Point(f64, f64),
    Finished,
    Interrupted,
}

unsafe fn locate_device_point(dev: pDevDesc) -> DevicePoint {
    let console = Console::get();
    let dc = Rc::clone(console.device_context());

    match dc.locate(console) {
        Ok(LocateOutcome::Point(point)) if point.x.is_finite() && point.y.is_finite() => {
            // Convert fractions of the plot image to device coordinates. The
            // bounds are the first fields of every `DevDesc` version. `top` is
            // the device coordinate of the top edge, so this is correct for
            // either y direction.
            let bounds = dev as *mut libr::DevDescBounds;
            let x = (*bounds).left + point.x * ((*bounds).right - (*bounds).left);
            let y = (*bounds).top + point.y * ((*bounds).bottom - (*bounds).top);
            DevicePoint::Point(x, y)
        },
        Ok(LocateOutcome::Point(point)) => {
            log::warn!("Ignoring non-finite plot point from the frontend: {point:?}");
            DevicePoint::Finished
        },
        Ok(LocateOutcome::Finished) => DevicePoint::Finished,
        Ok(LocateOutcome::Interrupted) => DevicePoint::Interrupted,
        Err(err) => {
            log::warn!("Can't locate a point on the plot: {err:?}");
            DevicePoint::Finished
        },
    }
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

        wrapped_callbacks.close.replace((*device).close);
        (*device).close = Some(callback_close);

        // Not wrapped: we report our own bounds, see `callback_size()`
        (*device).size = Some(callback_size);

        // Not wrapped: the wrapped devices write to files and have no usable
        // locator. `haveLocator` is 2 for "yes" and 1 for "no". Only console
        // sessions have a Plots pane to click on.
        (*device).locator = Some(callback_locator);
        (*device).haveLocator = match Console::get().session_mode() {
            SessionMode::Console => 2,
            _ => 1,
        };
    });

    let dc = Console::get().device_context();
    dc.device.set(Some(ge_device));

    // Apply a pane size that arrived before the device was created. Without
    // one, keep the device's own size.
    if let Some(size) = dc.pane_size.get() {
        dc.resize_device(size);
    }

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
/// when we drop into our [DeviceContext::hook_new_page] hook it will be too late for us to
/// record any changes because the display list we record gets cleared before our hook is
/// called.
///
/// It is also worth noting that we don't always drop into our [DeviceContext::hook_new_page]
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
/// not trigger a [DeviceContext::hook_new_page] hook. That's correct, as it gives us a chance
/// to record the intermediate plot, and then after `plot(2:20)` finishes it overwrites
/// that intermediate plot since we are still on the same plot page with the same plot
/// `id`.
#[tracing::instrument(level = "trace", skip_all)]
#[ark::register]
fn ps_graphics_before_plot_new(console: &Console, _name: SEXP) -> anyhow::Result<SEXP> {
    log::trace!("Entering ps_graphics_before_plot_new");

    // Process changes related to the last plot before opening a new page.
    // Particularly important if we make multiple plots in a single chunk.
    Rc::clone(console.device_context()).process_changes(console);

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

/// Replay the Ark device's display list. Called only from
/// `.ps.graphics.replay_quietly()`, see [DeviceContext::set_pane_size].
#[harp::register]
unsafe extern "C-unwind" fn ps_graphics_play_display_list() -> anyhow::Result<SEXP> {
    let Some(ge_device) = Console::get().device_context().device.get() else {
        return Ok(R_NilValue);
    };
    // The replay evaluates R code, so catch R errors here rather than
    // longjumping over this Rust frame
    harp::try_catch(|| libr::GEplayDisplayList(ge_device))?;
    Ok(R_NilValue)
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
    use amalthea::comm::plot_comm::RenderParams;

    use super::*;

    fn test_device_context() -> DeviceContext {
        let (tx, _rx) = crossbeam::channel::unbounded();
        DeviceContext::new(tx)
    }

    #[test]
    fn test_render_request_invalidates_last_prerender() {
        let dc = test_device_context();
        let id = PlotId(String::from("plot-1"));
        let size = PlotSize {
            width: 400,
            height: 300,
        };
        dc.last_prerender.replace(Some((id.clone(), size)));

        // Without a size or an intrinsic size the render fails before calling
        // into R, after the frontend asked for it
        let request = PlotBackendRequest::Render(RenderParams {
            size: None,
            pixel_ratio: 1.0,
            format: PlotRenderFormat::Png,
        });
        assert!(dc.handle_rpc(request, &id).is_err());
        assert!(dc.last_prerender.borrow().is_none());
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

    #[test]
    fn test_hook_mode_input_is_not_a_change() {
        let dc = test_device_context();

        // Mode 2 is graphical input (`locator()`), not drawing
        dc.hook_mode(2);
        assert_eq!(dc.state.borrow().changed_page(), None);

        dc.hook_mode(1);
        assert_eq!(dc.state.borrow().changed_page(), Some(&dc.id()));
    }
}
