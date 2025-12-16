//
// console_debug.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use anyhow::anyhow;
use anyhow::Result;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::protect::RProtect;
use harp::r_string;
use harp::session::r_sys_calls;
use harp::session::r_sys_frames;
use harp::session::r_sys_functions;
use harp::srcref::SrcRef;
use harp::utils::r_is_null;
use libr::SEXP;
use regex::Regex;
use stdext::result::ResultExt;
use url::Url;

use crate::dap::dap::DapBackendEvent;
use crate::interface::DebugCallText;
use crate::interface::DebugCallTextKind;
use crate::interface::RMain;
use crate::modules::ARK_ENVS;
use crate::srcref::ark_uri;
use crate::thread::RThreadSafe;

#[derive(Debug)]
pub struct FrameInfo {
    pub id: i64,
    /// The name shown in the editor tab bar when this frame is viewed.
    pub source_name: String,
    /// The name shown in the stack frame UI when this frame is visible.
    pub frame_name: String,
    pub source: FrameSource,
    pub environment: Option<RThreadSafe<RObject>>,
    pub start_line: i64,
    pub start_column: i64,
    pub end_line: i64,
    pub end_column: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameSource {
    File(String),
    Text(String),
}

/// Version of `FrameInfo` that identifies the frame by value and doesn't keep a
/// reference to the environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameInfoId {
    pub source: FrameSource,
    pub start_line: i64,
    pub start_column: i64,
    pub end_line: i64,
    pub end_column: i64,
}

impl From<&FrameInfo> for FrameInfoId {
    fn from(info: &FrameInfo) -> Self {
        FrameInfoId {
            source: info.source.clone(),
            start_line: info.start_line,
            start_column: info.start_column,
            end_line: info.end_line,
            end_column: info.end_column,
        }
    }
}

impl RMain {
    pub(crate) fn debug_start(&mut self, debug_preserve_focus: bool) {
        match self.debug_stack_info() {
            Ok(stack) => {
                // Figure out whether we changed location since last time,
                // e.g. because the user evaluated an expression that hit
                // another breakpoint. In that case we do want to move
                // focus, even though the user didn't explicitly used a step
                // gesture. Our indication that we changed location is
                // whether the call stack looks the same as last time. This
                // is not 100% reliable as this heuristic might have false
                // negatives, e.g. if the control flow exited the current
                // context via condition catching and jumped back in the
                // debugged function.
                let stack_id: Vec<FrameInfoId> = stack.iter().map(|f| f.into()).collect();
                let same_stack = stack_id == self.debug_last_stack;

                // Initialize fallback sources for this stack
                let fallback_sources = self.load_fallback_sources(&stack);

                self.debug_last_stack = stack_id;

                let preserve_focus = same_stack && debug_preserve_focus;

                let mut dap = self.debug_dap.lock().unwrap();
                dap.start_debug(stack, preserve_focus, fallback_sources)
            },
            Err(err) => log::error!("ReadConsole: Can't get stack info: {err}"),
        };
    }

    pub(crate) fn debug_stop(&mut self) {
        self.debug_last_stack = vec![];
        self.clear_fallback_sources();
        self.debug_reset_frame_id();
        self.debug_session_index += 1;

        let mut dap = self.debug_dap.lock().unwrap();
        dap.stop_debug();
    }

    pub(crate) fn debug_send_dap(&self, event: DapBackendEvent) {
        let dap = self.debug_dap.lock().unwrap();
        if let Some(tx) = &dap.backend_events_tx {
            tx.send(event).log_err();
        }
    }

    pub(crate) fn debug_handle_read_console(&mut self) {
        // Upon entering read-console, finalize any debug call text that we were capturing.
        // At this point, the user can either advance the debugger, causing us to capture
        // a new expression, or execute arbitrary code, where we will reuse a finalized
        // debug call text to maintain the debug state.
        match &self.debug_call_text {
            // If not debugging, nothing to do.
            DebugCallText::None => (),
            // If already finalized, keep what we have.
            DebugCallText::Finalized(_, _) => (),
            // If capturing, transition to finalized.
            DebugCallText::Capturing(call_text, kind) => {
                self.debug_call_text = DebugCallText::Finalized(call_text.clone(), *kind)
            },
        }
    }

    pub(crate) fn debug_handle_write_console(&mut self, content: &str) {
        if let DebugCallText::Capturing(ref mut call_text, _) = self.debug_call_text {
            // Append to current expression if we are currently capturing stdout
            call_text.push_str(content);
            return;
        }

        // `debug: ` is emitted by R (if no srcrefs are available!) right before it emits
        // the current expression we are debugging, so we use that as a signal to begin
        // capturing.
        if content == "debug: " {
            self.debug_call_text =
                DebugCallText::Capturing(String::new(), DebugCallTextKind::Debug);
            return;
        }

        // `debug at *PATH*: *EXPR*` is emitted by R when stepping through
        // blocks that have srcrefs. We use this to detect that we've just
        // stepped to an injected breakpoint and need to move on automatically.
        if content.starts_with("debug at ") {
            self.debug_call_text =
                DebugCallText::Capturing(String::new(), DebugCallTextKind::DebugAt);
            return;
        }

        // Entering or exiting a closure, reset the debug start line state and call text
        if content == "debugging in: " || content == "exiting from: " {
            self.debug_last_line = None;
            self.debug_call_text = DebugCallText::None;
            return;
        }
    }

    pub(crate) fn debug_stack_info(&mut self) -> Result<Vec<FrameInfo>> {
        // We leave finalized `call_text` in place rather than setting it to `None` here
        // in case the user executes an arbitrary expression in the debug R console, which
        // loops us back here without updating the `call_text` in any way, allowing us to
        // recreate the debugger state after their code execution.
        let call_text = match self.debug_call_text.clone() {
            DebugCallText::None => None,
            DebugCallText::Capturing(call_text, _) => {
                log::error!(
                    "Call text is in `Capturing` state, but should be `Finalized`: '{call_text}'."
                );
                None
            },
            DebugCallText::Finalized(call_text, DebugCallTextKind::Debug) => Some(call_text),
            DebugCallText::Finalized(_, DebugCallTextKind::DebugAt) => None,
        };

        let last_start_line = self.debug_last_line;

        let frames = self.debug_r_stack_info(call_text, last_start_line)?;

        // If we have `frames`, update the `last_start_line` with the context
        // frame's start line
        if let Some(frame) = frames.get(0) {
            self.debug_last_line = Some(frame.start_line);
        }

        Ok(frames)
    }

    pub(crate) fn debug_r_stack_info(
        &mut self,
        context_call_text: Option<String>,
        context_last_start_line: Option<i64>,
    ) -> Result<Vec<FrameInfo>> {
        unsafe {
            let mut protect = RProtect::new();

            let context_srcref = libr::get(libr::R_Srcref);
            protect.add(context_srcref);

            let context_call_text = match context_call_text {
                Some(context_call_text) => r_string!(context_call_text, &mut protect),
                None => libr::R_NilValue,
            };

            let context_last_start_line = match context_last_start_line {
                Some(context_last_start_line) => {
                    let x = libr::Rf_allocVector(libr::INTSXP, 1);
                    protect.add(x);
                    libr::SET_INTEGER_ELT(x, 0, i32::try_from(context_last_start_line)?);
                    x
                },
                None => libr::R_NilValue,
            };

            let functions = r_sys_functions()?;
            protect.add(functions);

            let environments = r_sys_frames()?;
            protect.add(environments.sexp);

            let calls = r_sys_calls()?;
            protect.add(calls.sexp);

            let info = RFunction::new("", "debugger_stack_info")
                .add(context_call_text)
                .add(context_last_start_line)
                .add(context_srcref)
                .add(functions)
                .add(environments)
                .add(calls)
                .call_in(ARK_ENVS.positron_ns)?;

            let n: isize = libr::Rf_xlength(info.sexp);

            let mut out = Vec::with_capacity(n as usize);

            // Reverse the order for DAP
            for i in (0..n).rev() {
                let frame = libr::VECTOR_ELT(info.sexp, i);
                let id = self.debug_next_frame_id();
                out.push(as_frame_info(frame, id)?);
            }

            log::trace!("DAP: Current call stack:\n{out:#?}");

            Ok(out)
        }
    }

    fn debug_next_frame_id(&mut self) -> i64 {
        let out = self.debug_current_frame_id;
        self.debug_current_frame_id += 1;
        out
    }

    pub(crate) fn debug_reset_frame_id(&mut self) {
        self.debug_current_frame_id = 0;
    }

    pub(crate) fn ark_debug_uri(
        debug_session_index: u32,
        source_name: &str,
        source: &str,
    ) -> String {
        // Hash the source to generate a unique identifier used in
        // the URI. This is needed to disambiguate frames that have
        // the same source name (used as file name in the URI) but
        // different sources.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hash;
        use std::hash::Hasher;
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        let hash = format!("{:x}", hasher.finish());

        ark_uri(&format!(
            "debug/session{i}/{hash}/{source_name}",
            i = debug_session_index,
        ))
    }

    // Doesn't expect `ark:` scheme, used for checking keys in our vdoc map
    pub(crate) fn is_ark_debug_path(uri: &str) -> bool {
        static RE_ARK_DEBUG_URI: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let re = RE_ARK_DEBUG_URI.get_or_init(|| Regex::new(r"^ark-\d+/debug/").unwrap());
        re.is_match(uri)
    }

    pub(crate) fn verify_breakpoints(&self, srcref: RObject) {
        let Some(srcref) = SrcRef::try_from(srcref).warn_on_err() else {
            return;
        };

        let Some(filename) = srcref
            .srcfile()
            .and_then(|srcfile| srcfile.filename())
            .log_err()
        else {
            return;
        };

        // Only process file:// URIs (from our #line directives).
        // Plain file paths or empty filenames are skipped silently.
        if !filename.starts_with("file://") {
            return;
        }

        let Some(uri) = Url::parse(&filename).warn_on_err() else {
            return;
        };

        let mut dap = self.debug_dap.lock().unwrap();
        dap.verify_breakpoints(&uri, srcref.line_virtual.start, srcref.line_virtual.end);
    }
}

fn as_frame_info(info: libr::SEXP, id: i64) -> Result<FrameInfo> {
    unsafe {
        let mut i = 0;

        let source_name = libr::VECTOR_ELT(info, i);
        let source_name: String = RObject::view(source_name).try_into()?;

        i += 1;
        let frame_name = libr::VECTOR_ELT(info, i);
        let frame_name: String = RObject::view(frame_name).try_into()?;

        let mut source = None;

        i += 1;
        let file = libr::VECTOR_ELT(info, i);
        if file != libr::R_NilValue {
            let file: String = RObject::view(file).try_into()?;
            source = Some(FrameSource::File(file));
        }

        i += 1;
        let text = libr::VECTOR_ELT(info, i);
        if text != libr::R_NilValue {
            let text: String = RObject::view(text).try_into()?;
            source = Some(FrameSource::Text(text));
        }

        let Some(source) = source else {
            return Err(anyhow!(
                "Expected either `file` or `text` to be non-`NULL`."
            ));
        };

        i += 1;
        let environment = libr::VECTOR_ELT(info, i);
        let environment = if r_is_null(environment) {
            None
        } else {
            Some(RThreadSafe::new(RObject::from(environment)))
        };

        i += 1;
        let start_line = libr::VECTOR_ELT(info, i);
        let start_line: i32 = RObject::view(start_line).try_into()?;

        i += 1;
        let start_column = libr::VECTOR_ELT(info, i);
        let start_column: i32 = RObject::view(start_column).try_into()?;

        i += 1;
        let end_line = libr::VECTOR_ELT(info, i);
        let end_line: i32 = RObject::view(end_line).try_into()?;

        // For `end_column`, the column range provided by R is inclusive `[,]`, but the
        // one used on the DAP / Positron side is exclusive `[,)` so we have to add 1.
        i += 1;
        let end_column = libr::VECTOR_ELT(info, i);
        let end_column: i32 = RObject::view(end_column).try_into()?;
        let end_column = end_column + 1;

        Ok(FrameInfo {
            id,
            source_name,
            frame_name,
            source,
            environment,
            start_line: start_line.try_into()?,
            start_column: start_column.try_into()?,
            end_line: end_line.try_into()?,
            end_column: end_column.try_into()?,
        })
    }
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_is_breakpoint_enabled(
    uri: SEXP,
    id: SEXP,
) -> anyhow::Result<SEXP> {
    let uri: String = RObject::view(uri).try_into()?;
    let uri = Url::parse(&uri)?;

    let id: String = RObject::view(id).try_into()?;

    let console = RMain::get_mut();
    let dap = console.debug_dap.lock().unwrap();

    let enabled: RObject = dap.is_breakpoint_enabled(&uri, id).into();
    Ok(enabled.sexp)
}

/// Verify breakpoints in the line range covered by a srcref.
/// Called after each expression is successfully evaluated in source().
#[harp::register]
pub unsafe extern "C-unwind" fn ps_verify_breakpoints(srcref: SEXP) -> anyhow::Result<SEXP> {
    let srcref = RObject::view(srcref);
    RMain::get().verify_breakpoints(srcref.clone());
    Ok(libr::R_NilValue)
}

/// Verify breakpoints in an explicit line range.
/// Called after each top-level expression in source() when using the source hook.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_verify_breakpoints_range(
    uri: SEXP,
    start_line: SEXP,
    end_line: SEXP,
) -> anyhow::Result<SEXP> {
    let uri: String = RObject::view(uri).try_into()?;
    let start_line: i32 = RObject::view(start_line).try_into()?;
    let end_line: i32 = RObject::view(end_line).try_into()?;

    let Ok(uri) = Url::parse(&uri) else {
        return Ok(libr::R_NilValue);
    };

    let main = RMain::get();
    let mut dap = main.debug_dap.lock().unwrap();
    dap.verify_breakpoints(&uri, start_line as u32, end_line as u32);

    Ok(libr::R_NilValue)
}
