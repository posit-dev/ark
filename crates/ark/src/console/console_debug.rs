//
// console_debug.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::stream::Stream;
use amalthea::wire::stream::StreamOutput;
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

use crate::console::Console;
use crate::modules::ARK_ENVS;
use crate::srcref::ark_uri;
use crate::thread::RThreadSafe;
use crate::url::UrlId;

/// Debug call text captured from R's debug output.
#[derive(Clone, Debug)]
pub(crate) enum DebugCallText {
    /// `debug: <expr>` - emitted when stepping without srcrefs
    Debug(String),
    /// `debug at <path>#<line>: <expr>` - emitted when stepping with srcrefs
    DebugAt(String),
}

#[derive(Debug, Clone)]
pub(crate) enum DebugStoppedReason {
    Step,
    Pause,
    Condition { class: String, message: String },
}

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
pub(super) struct FrameInfoId {
    source: FrameSource,
    start_line: i64,
    start_column: i64,
    end_line: i64,
    end_column: i64,
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

impl Console {
    pub(super) fn debug_start(
        &mut self,
        transient_eval: bool,
        debug_stopped_reason: DebugStoppedReason,
    ) {
        match self.debug_stack_info() {
            Ok(mut stack) => {
                // Figure out whether we changed location since last time,
                // e.g. because a transient eval hit another breakpoint. In
                // that case we can't preserve the debug session and need to
                // show the user the new location. Our indication that we
                // changed location is whether the call stack looks the same
                // as last time. This is not 100% reliable as this heuristic
                // might have false negatives, e.g. if the control flow
                // exited the current context via condition catching and
                // jumped back in the debugged function.
                let stack_id: Vec<FrameInfoId> = stack.iter().map(|f| f.into()).collect();
                let stack_changed = stack_id != self.debug_last_stack;

                // Transient eval with unchanged stack: just refresh variables
                if transient_eval && !stack_changed {
                    let dap = self.debug_dap.lock().unwrap();
                    dap.send_invalidated();
                    return;
                }

                // If we skipped `debug_stop` during a transient eval but the
                // stack changed, clean up and notify frontend before starting
                // the new session.
                if transient_eval {
                    self.debug_stop_session();
                }

                self.debug_last_stack = stack_id;

                // Initialize fallback sources for this stack
                let fallback_sources = self.load_fallback_sources(&stack);

                let show = get_show_hidden_frames();
                if !show.internal {
                    remove_condition_handling_frames(&mut stack, &debug_stopped_reason);
                }
                if !show.fenced {
                    remove_fenced_frames(&mut stack);
                }

                let mut dap = self.debug_dap.lock().unwrap();
                dap.start_debug(stack, fallback_sources, debug_stopped_reason)
            },
            Err(err) => log::error!("ReadConsole: Can't get stack info: {err:?}"),
        };
    }

    pub(super) fn debug_stop(&mut self) {
        // Preserve all state in case of transient eval. Only guard when
        // actually debugging, otherwise we skip resetting state like
        // `is_interrupting_for_debugger` that needs cleanup regardless.
        if self.debug_is_debugging && self.debug_transient_eval {
            return;
        }

        self.debug_is_debugging = false;
        self.debug_stopped_reason = None;
        self.debug_last_stack = vec![];
        self.debug_call_text = None;
        self.debug_last_line = None;
        self.debug_stop_session();
    }

    fn debug_stop_session(&mut self) {
        self.clear_fallback_sources();
        self.debug_session_index += 1;
        self.set_debug_selected_frame_id(None);

        let mut dap = self.debug_dap.lock().unwrap();
        dap.stop_debug();
    }

    fn debug_stack_info(&mut self) -> Result<Vec<FrameInfo>> {
        // We leave finalized `call_text` in place rather than setting it to `None` here
        // in case the user executes an arbitrary expression in the debug R console, which
        // loops us back here without updating the `call_text` in any way, allowing us to
        // recreate the debugger state after their code execution.
        let call_text = match &self.debug_call_text {
            None => None,
            Some(DebugCallText::Debug(text)) => Some(text.clone()),
            Some(DebugCallText::DebugAt(_)) => None,
        };

        let last_start_line = self.debug_last_line;

        let frames = self.debug_r_stack_info(call_text, last_start_line)?;

        // If we have `frames`, update the `last_start_line` with the context
        // frame's start line
        if let Some(frame) = frames.first() {
            self.debug_last_line = Some(frame.start_line);
        }

        Ok(frames)
    }

    fn debug_r_stack_info(
        &mut self,
        context_call_text: Option<String>,
        context_last_start_line: Option<i64>,
    ) -> Result<Vec<FrameInfo>> {
        unsafe {
            let mut protect = RProtect::new();

            // R_Srcref can be a C NULL pointer (0x0) in certain contexts, such
            // as when `browser()` is called inside `dplyr::mutate()`. Convert
            // NULL pointers to R_NilValue to avoid the crash. See
            // https://github.com/posit-dev/positron/issues/8979
            let srcref = libr::get(libr::R_Srcref);
            let context_srcref = if srcref.is_null() {
                libr::R_NilValue
            } else {
                srcref
            };
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

            // The captured current environment may differ from
            // `sys.frame(sys.nframe())` when evaluating in a promise
            // environment. Pass it so we can add a synthetic frame to the
            // stack.
            let current_env = self.eval_env();

            let info = RFunction::new("", "debugger_stack_info")
                .add(context_call_text)
                .add(context_last_start_line)
                .add(context_srcref)
                .add(functions)
                .add(environments.sexp)
                .add(calls.sexp)
                .add(current_env)
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

    pub(super) fn ark_debug_uri(
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
    pub(super) fn is_ark_debug_path(uri: &str) -> bool {
        static RE_ARK_DEBUG_URI: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let re = RE_ARK_DEBUG_URI.get_or_init(|| Regex::new(r"^ark-\d+/debug/").unwrap());
        re.is_match(uri)
    }

    pub(super) fn verify_breakpoints(&self, srcref: RObject) {
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

        let Some(uri) = UrlId::parse(&filename).warn_on_err() else {
            return;
        };

        let mut dap = self.debug_dap.lock().unwrap();
        dap.verify_breakpoints(&uri, srcref.line_virtual.start, srcref.line_virtual.end);
    }
}

/// Controls which categories of hidden frames to show (i.e., not filter out).
/// Parsed from the `ark.debugger.show_hidden_frames` R option.
struct ShowHiddenFrames {
    /// Show frames fenced by `..stacktraceon..`/`..stacktraceoff..` sentinels
    fenced: bool,
    /// Show internal condition-handling frames (e.g. `.handleSimpleError()`)
    internal: bool,
}

/// Read the `ark.debugger.show_hidden_frames` R option via the R-side
/// `debugger_show_hidden_frames()` helper, which validates and normalises
/// the option into a character vector of categories to show.
fn get_show_hidden_frames() -> ShowHiddenFrames {
    let values =
        match RFunction::new("", "debugger_show_hidden_frames").call_in(ARK_ENVS.positron_ns) {
            Ok(obj) => Vec::<String>::try_from(obj).unwrap_or_default(),
            Err(err) => {
                log::warn!("Failed to read `ark.debugger.show_hidden_frames`: {err:?}");
                return ShowHiddenFrames {
                    fenced: false,
                    internal: false,
                };
            },
        };

    ShowHiddenFrames {
        fenced: values.iter().any(|v| v == "fenced"),
        internal: values.iter().any(|v| v == "internal"),
    }
}

/// Discard top frames that are part of the debug infrastructure rather than
/// user code.
fn remove_condition_handling_frames(
    stack: &mut Vec<FrameInfo>,
    stopped_reason: &DebugStoppedReason,
) {
    // Discard top frame when stopped due to exception breakpoint or pause,
    // it points to our global handler that calls `browser()`
    if matches!(
        stopped_reason,
        DebugStoppedReason::Condition { .. } | DebugStoppedReason::Pause
    ) && !stack.is_empty()
    {
        stack.remove(0);
    }

    // Then discard base R's own condition handling/emitting frames, if any
    remove_frame_prefix(stack, &[".handleSimpleError()"]);
    remove_frame_prefix(stack, &[
        "doWithOneRestart()",
        "withOneRestart()",
        "withRestarts()",
        ".signalSimpleWarning",
    ]);
}

/// Remove frames from the top of the stack that match the given prefixes in order.
fn remove_frame_prefix(stack: &mut Vec<FrameInfo>, prefixes: &[&str]) {
    for prefix in prefixes {
        if stack
            .first()
            .is_some_and(|frame| frame.frame_name.starts_with(prefix))
        {
            stack.remove(0);
        } else {
            break;
        }
    }
}

/// Removes frames fenced between `..stacktraceon..` and `..stacktraceoff..`
/// markers (used by Shiny to hide internal frames from error stack traces).
///
/// Shiny's filtering uses a score-based system (see `stripOneStackTrace` in
/// Shiny's R/conditions.R): score starts at 1, `..stacktraceon..` adds 1,
/// `..stacktraceoff..` subtracts 1, and frames with score < 1 are hidden.
/// Sentinels are expected to be properly nested like parentheses.
///
/// Since our stack is innermost-first (opposite to Shiny's traversal), the
/// semantics invert: `..stacktraceon..` *enters* a hidden region (going
/// outward) and `..stacktraceoff..` *exits* it. We use `hidden_depth` as an
/// equivalent to Shiny's score, where `depth == 0` means visible.
///
/// Note: Shiny also uses `..stacktracefloor..` to truncate stacks entirely
/// below that point. We don't handle this since showing the full context
/// (e.g. `shiny::runApp()`) is useful in a debugger. Shiny's
/// `shiny.fullstacktrace` option disables filtering; our equivalent is
/// `ark.debugger.show_hidden_frames`.
///
/// The topmost frame (index 0) is never filtered out so the user always
/// sees where they are stopped.
fn remove_fenced_frames(frames: &mut Vec<FrameInfo>) {
    let mut hidden_depth: u32 = 0;
    let mut first = true;

    // `Vec::retain` iterates front-to-back (guaranteed by std)
    frames.retain(|frame| {
        // Always preserve the topmost frame so the user sees where they're stopped
        if first {
            first = false;
            return true;
        }
        // Frame names are formatted as `fn_name()`, match on the prefix
        if frame.frame_name.starts_with("..stacktraceon..") {
            hidden_depth += 1;
            return false;
        }
        if frame.frame_name.starts_with("..stacktraceoff..") {
            if hidden_depth == 0 {
                log::trace!(
                    "Unbalanced `..stacktraceoff..` without prior `..stacktraceon..` in call stack"
                );
            }
            hidden_depth = hidden_depth.saturating_sub(1);
            return false;
        }
        hidden_depth == 0
    });

    if hidden_depth > 0 {
        log::warn!(
            "Unmatched `..stacktraceon..` without closing `..stacktraceoff..` in call stack"
        );
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
            start_line: start_line.into(),
            start_column: start_column.into(),
            end_line: end_line.into(),
            end_column: end_column.into(),
        })
    }
}

/// Check whether a breakpoint should actually stop execution.
///
/// Combines the enabled check with condition evaluation in a single call
#[harp::register]
pub unsafe extern "C-unwind" fn ps_handle_breakpoint(
    uri: SEXP,
    id: SEXP,
    env: SEXP,
) -> anyhow::Result<SEXP> {
    let env = RObject::new(env);

    let uri: String = RObject::view(uri).try_into()?;
    let uri = UrlId::parse(&uri)?;

    let id: String = RObject::view(id).try_into()?;
    let id: i64 = id.parse()?;

    let console = Console::get_mut();
    let dap = console.debug_dap.lock().unwrap();

    let enabled = dap.is_breakpoint_enabled(&uri, id);
    let bp = dap.get_breakpoint(&uri, id);
    let bp_line = bp.map_or(0, |bp| bp.line);
    let condition = bp.and_then(|bp| bp.condition.clone());
    let log_message = bp.and_then(|bp| bp.log_message.clone());
    let hit_condition = bp.and_then(|bp| bp.hit_condition.clone());

    log::trace!(
        "DAP: Breakpoint {id} for {uri} \
         enabled: {enabled}, \
         hit_count: {hit_count}, \
         hit_condition: {hit_condition:?}, \
         condition: {condition:?}, \
         log_message: {log_message:?}",
        hit_count = bp.map_or(0, |bp| bp.hit_count)
    );

    if !enabled {
        return Ok(RObject::from(false).sexp);
    }

    // Must drop before calling back into R to avoid deadlock
    drop(dap);

    // Evaluate condition first as it applies to all breakpoints, including log
    // and hit-count breakpoints. Per the DAP spec, `hitCondition` should only
    // be evaluated (and the hit count incremented) if the `condition` is met.
    let should_break = match &condition {
        None => true,
        Some(condition) => {
            let ((should_break, error), captured_output) =
                Console::with_capture(|| eval_condition(condition, env.clone()));

            if !captured_output.trim().is_empty() || error.is_some() {
                let mut output = format!("Code: `{condition}`\n");
                output.push_str(&captured_output);
                if let Some(err) = error {
                    output.push_str(&err);
                }
                emit_breakpoint_block(&uri, bp_line, &output);
            }
            should_break
        },
    };

    if !should_break {
        return Ok(RObject::from(false).sexp);
    }

    if let Some(ref hit_condition) = hit_condition {
        match hit_condition.trim().parse::<u64>() {
            Ok(threshold) => {
                let mut dap = Console::get_mut().debug_dap.lock().unwrap();
                let hit_count = dap.increment_hit_count(&uri, id);
                drop(dap);

                if hit_count < threshold {
                    return Ok(RObject::from(false).sexp);
                }
            },
            Err(err) => {
                emit_breakpoint_block(
                    &uri,
                    bp_line,
                    &format!("Error: Expected a positive integer, {err}"),
                );
            },
        }
    }

    // Log breakpoints evaluate the template and never stop
    if let Some(log_message) = log_message {
        let (output, captured_output) =
            Console::with_capture(|| eval_log_message(&log_message, env));

        let mut all_output = captured_output;
        all_output.push_str(&output);
        if !all_output.is_empty() && !all_output.ends_with('\n') {
            all_output.push('\n');
        }

        emit_breakpoint_block(&uri, bp_line, &all_output);
        return Ok(RObject::from(false).sexp);
    }

    Ok(RObject::from(true).sexp)
}

/// Emit a fenced breakpoint block to stderr.
fn emit_breakpoint_block(uri: &UrlId, line: u32, content: &str) {
    let Some(text) = format_breakpoint_block(uri, line, content) else {
        return;
    };

    Console::get_mut()
        .iopub_tx()
        .send(IOPubMessage::Stream(StreamOutput {
            name: Stream::Stderr,
            text,
        }))
        .unwrap();
}

fn format_breakpoint_block(uri: &UrlId, line: u32, content: &str) -> Option<String> {
    if content.trim().is_empty() {
        return None;
    }

    let label = breakpoint_label(uri, line);

    let mut text = format!("```breakpoint {label}\n");

    text.push_str(content);
    if !content.ends_with('\n') {
        text.push('\n');
    }

    text.push_str("```\n");

    Some(text)
}

/// Evaluate a DAP log message template. Uses `glue::glue()` for `{expression}`
/// interpolation (mandated by DAP) if glue is installed, otherwise returns the
/// template as-is.
fn eval_log_message(template: &str, env: RObject) -> String {
    match RFunction::new("base", ".ark_eval_log_message")
        .add(RObject::from(template))
        .call_in(env.sexp)
    {
        Ok(val) => String::try_from(val).unwrap_or_default(),
        Err(harp::Error::TryCatchError(err)) => format!("Error: {}", err.message),
        Err(err) => format!("Error: {err}"),
    }
}

/// Format a clickable `filename#line` label for breakpoint output.
fn breakpoint_label(uri: &UrlId, line: u32) -> String {
    let filename = uri
        .as_url()
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("unknown");
    let display_line = line + 1;
    let display = format!("{filename}#{display_line}");
    format!("\x1b]8;line={display_line};{uri}\x07{display}\x1b]8;;\x07")
}

/// Evaluate a condition expression in a given environment.
///
/// Returns `(should_break, error)`. Warnings and messages are captured by
/// `Console::with_capture` at the call site (which sets `warn = 1` so
/// warnings are emitted immediately). When evaluation fails or
/// produces a non-logical result, `should_break` is `true` so that typos
/// in conditions cause a visible stop rather than a silently ignored
/// breakpoint.
fn eval_condition(condition: &str, envir: RObject) -> (bool, Option<String>) {
    // `if` coerces via `asLogicalNoNA` (not the generic `as.logical`)
    // and errors on NA, length != 1, and non-coercible types.
    let code = format!("if ({{ {condition} }}) TRUE else FALSE");

    let result = match harp::parse_eval0(&code, envir) {
        Ok(val) => val,
        Err(harp::Error::TryCatchError(err)) => {
            return (true, Some(format!("Error: {}\n", err.message)));
        },
        Err(err) => {
            return (true, Some(format!("Error: {err}\n")));
        },
    };

    match bool::try_from(RObject::view(result.sexp)) {
        Ok(val) => (val, None),
        Err(err) => (true, Some(format!("Error: {err}\n"))),
    }
}

/// Verify a single breakpoint by ID.
/// Called when a breakpoint expression is about to be evaluated.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_verify_breakpoint(uri: SEXP, id: SEXP) -> anyhow::Result<SEXP> {
    let uri: String = RObject::view(uri).try_into()?;
    let id: String = RObject::view(id).try_into()?;

    let Some(uri) = UrlId::parse(&uri).log_err() else {
        return Ok(libr::R_NilValue);
    };

    let mut dap = Console::get().debug_dap.lock().unwrap();
    dap.verify_breakpoint(&uri, &id);

    Ok(libr::R_NilValue)
}

/// Verify breakpoints in the line range covered by a srcref.
/// Called after each expression is successfully evaluated in source().
#[harp::register]
pub unsafe extern "C-unwind" fn ps_verify_breakpoints(srcref: SEXP) -> anyhow::Result<SEXP> {
    Console::get().verify_breakpoints(RObject::view(srcref));
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

    let Some(uri) = UrlId::parse(&uri).log_err() else {
        return Ok(libr::R_NilValue);
    };

    let mut dap = Console::get().debug_dap.lock().unwrap();
    dap.verify_breakpoints(&uri, start_line as u32, end_line as u32);

    Ok(libr::R_NilValue)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_debug_should_break_on_condition(
    filter: SEXP,
) -> anyhow::Result<SEXP> {
    let filter: String = RObject::view(filter).try_into()?;

    let console = Console::get_mut();
    let dap = console.debug_dap.lock().unwrap();

    let enabled: RObject = dap.is_exception_breakpoint_filter_enabled(&filter).into();
    Ok(enabled.sexp)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_debug_set_stopped_reason(
    class: SEXP,
    message: SEXP,
) -> anyhow::Result<SEXP> {
    let class: String = RObject::view(class).try_into()?;
    let message: String = RObject::view(message).try_into()?;

    Console::get_mut().debug_stopped_reason =
        Some(DebugStoppedReason::Condition { class, message });

    Ok(libr::R_NilValue)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_debug_set_stopped_reason_pause() -> anyhow::Result<SEXP> {
    Console::get_mut().debug_stopped_reason = Some(DebugStoppedReason::Pause);
    Ok(libr::R_NilValue)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_is_interrupting_for_debugger() -> anyhow::Result<SEXP> {
    let console = Console::get_mut();
    let mut dap = console.debug_dap.lock().unwrap();

    let result: RObject = dap.is_interrupting_for_debugger.into();
    dap.is_interrupting_for_debugger = false;

    Ok(result.sexp)
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;

    fn frame(name: &str) -> FrameInfo {
        FrameInfo {
            id: 0,
            source_name: String::new(),
            frame_name: name.to_string(),
            source: FrameSource::Text(String::new()),
            environment: None,
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
        }
    }

    fn names(frames: &[FrameInfo]) -> Vec<&str> {
        frames.iter().map(|f| f.frame_name.as_str()).collect()
    }

    fn test_uri(path: &str) -> UrlId {
        UrlId::from_url(Url::parse(&format!("file:///project/{path}")).unwrap())
    }

    #[test]
    fn test_filter_hidden_frames_empty() {
        let mut frames = vec![];
        remove_fenced_frames(&mut frames);
        assert!(frames.is_empty());
    }

    #[test]
    fn test_filter_hidden_frames_no_sentinels() {
        let mut frames = vec![frame("a()"), frame("b()"), frame("c()")];
        remove_fenced_frames(&mut frames);
        assert_eq!(names(&frames), vec!["a()", "b()", "c()"]);
    }

    #[test]
    fn test_filter_hidden_frames_basic_region() {
        let mut frames = vec![
            frame("user_code()"),
            frame("..stacktraceon..()"),
            frame("shiny_internal()"),
            frame("..stacktraceoff..()"),
            frame("outer()"),
        ];
        remove_fenced_frames(&mut frames);
        assert_eq!(names(&frames), vec!["user_code()", "outer()"]);
    }

    #[test]
    fn test_filter_hidden_frames_sequential_regions() {
        let mut frames = vec![
            frame("user_code()"),
            frame("..stacktraceon..()"),
            frame("inner_on()"),
            frame("internal_a()"),
            frame("..stacktraceoff..()"),
            frame("inner_off()"),
            frame("middle_user()"),
            frame("..stacktraceon..()"),
            frame("outer_on()"),
            frame("internal_b()"),
            frame("..stacktraceoff..()"),
            frame("outer_off()"),
            frame("top()"),
        ];
        remove_fenced_frames(&mut frames);
        assert_eq!(names(&frames), vec![
            "user_code()",
            "inner_off()",
            "middle_user()",
            "outer_off()",
            "top()"
        ]);
    }

    #[test]
    fn test_filter_hidden_frames_nested_regions() {
        // Shiny nests sentinel pairs. Inner `..stacktraceon..` increases depth,
        // and we only exit hidden mode when depth returns to 0.
        let mut frames = vec![
            frame("renderPlot()"),
            frame("..stacktraceon..(renderPlot())"),
            frame("func()"),
            frame("..stacktraceon..(<reactive:plotObj>)"),
            frame("internal_deep()"),
            frame("..stacktraceoff..(self$.updateValue())"),
            frame("still_hidden()"),
            frame("..stacktraceoff..(renderFunc)"),
            frame("output$distPlot()"),
            frame("..stacktraceon..(output$distPlot)"),
            frame("more_internal()"),
            frame("..stacktraceoff..(captureStackTraces)"),
            frame("shiny::runApp()"),
        ];
        remove_fenced_frames(&mut frames);
        assert_eq!(names(&frames), vec![
            "renderPlot()",
            "output$distPlot()",
            "shiny::runApp()"
        ]);
    }

    #[test]
    fn test_filter_hidden_frames_deeply_nested() {
        // Multiple levels of nesting
        let mut frames = vec![
            frame("user()"),
            frame("..stacktraceon..()"),
            frame("a()"),
            frame("..stacktraceon..()"),
            frame("b()"),
            frame("..stacktraceon..()"),
            frame("c()"),
            frame("..stacktraceoff..()"),
            frame("d()"),
            frame("..stacktraceoff..()"),
            frame("e()"),
            frame("..stacktraceoff..()"),
            frame("visible()"),
        ];
        remove_fenced_frames(&mut frames);
        assert_eq!(names(&frames), vec!["user()", "visible()"]);
    }

    #[test]
    fn test_filter_hidden_frames_only_sentinels() {
        let mut frames = vec![frame("..stacktraceon..()"), frame("..stacktraceoff..()")];
        remove_fenced_frames(&mut frames);
        // The first frame is always kept, even if it's a sentinel
        assert_eq!(names(&frames), vec!["..stacktraceon..()"]);
    }

    #[test]
    fn test_filter_hidden_frames_lone_traceoff() {
        // A lone `..stacktraceoff..` is a no-op: it exits a hidden region
        // that was never entered, so all other frames remain visible.
        let mut frames = vec![
            frame("user_code()"),
            frame("..stacktraceoff..()"),
            frame("wrapper()"),
        ];
        remove_fenced_frames(&mut frames);
        assert_eq!(names(&frames), vec!["user_code()", "wrapper()"]);
    }

    #[test]
    fn test_filter_hidden_frames_topmost_preserved_when_unmatched() {
        // `..stacktraceon..` (not off) is correct here: in our innermost-first
        // scan it enters the hidden region, so a missing `..stacktraceoff..`
        // leaves everything above it hidden.
        let mut frames = vec![
            frame("user_code()"),
            frame("..stacktraceon..()"),
            frame("wrapper()"),
        ];
        remove_fenced_frames(&mut frames);
        assert_eq!(names(&frames), vec!["user_code()"]);
    }

    #[test]
    fn test_filter_hidden_frames_first_frame_is_sentinel() {
        // The topmost frame is always kept, even if it's a sentinel
        let mut frames = vec![
            frame("..stacktraceon..()"),
            frame("internal()"),
            frame("..stacktraceoff..()"),
            frame("outer()"),
        ];
        remove_fenced_frames(&mut frames);
        // The sentinel at index 0 is kept without triggering hidden state,
        // so `internal()` between on/off is also visible
        assert_eq!(names(&frames), vec![
            "..stacktraceon..()",
            "internal()",
            "outer()"
        ]);
    }

    #[test]
    fn test_format_breakpoint_block_nothing() {
        let uri = test_uri("test.R");
        assert_eq!(format_breakpoint_block(&uri, 2, ""), None);
        assert_eq!(format_breakpoint_block(&uri, 2, "  \n"), None);
    }

    #[test]
    fn test_format_breakpoint_block_error_only() {
        let uri = test_uri("test.R");
        let result =
            format_breakpoint_block(&uri, 2, "Code: `x > 1`\nError: object 'x' not found\n");
        let link = breakpoint_label(&uri, 2);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<test.R#3>"), @r"
        ```breakpoint <test.R#3>
        Code: `x > 1`
        Error: object 'x' not found
        ```
        ");
    }

    #[test]
    fn test_format_breakpoint_block_warning_only() {
        let uri = test_uri("test.R");
        let result = format_breakpoint_block(&uri, 4, "Code: `x > 1`\nWarning: something\n");
        let link = breakpoint_label(&uri, 4);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<test.R#5>"), @r"
        ```breakpoint <test.R#5>
        Code: `x > 1`
        Warning: something
        ```
        ");
    }

    #[test]
    fn test_format_breakpoint_block_with_error() {
        let uri = test_uri("analysis.R");
        let content = "Code: `nrow(df)`\nWarning message:\ncoercion applied\nError: Expected TRUE or FALSE, got 5\n";
        let result = format_breakpoint_block(&uri, 9, content);
        let link = breakpoint_label(&uri, 9);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<analysis.R#10>"), @r"
        ```breakpoint <analysis.R#10>
        Code: `nrow(df)`
        Warning message:
        coercion applied
        Error: Expected TRUE or FALSE, got 5
        ```
        ");
    }

    #[test]
    fn test_format_breakpoint_block_no_trailing_newline() {
        let uri = test_uri("test.R");
        let result = format_breakpoint_block(&uri, 0, "Code: `x > 1`\nWarning: oops");
        let link = breakpoint_label(&uri, 0);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<test.R#1>"), @r"
        ```breakpoint <test.R#1>
        Code: `x > 1`
        Warning: oops
        ```
        ");
    }

    #[test]
    fn test_format_breakpoint_block_log_output() {
        let uri = test_uri("test.R");
        let result = format_breakpoint_block(&uri, 2, "x is 42, y is hello\n");
        let link = breakpoint_label(&uri, 2);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<test.R#3>"), @r"
        ```breakpoint <test.R#3>
        x is 42, y is hello
        ```
        ");
    }

    #[test]
    fn test_format_breakpoint_block_log_output_no_trailing_newline() {
        let uri = test_uri("script.R");
        let result = format_breakpoint_block(&uri, 5, "iteration 3");
        let link = breakpoint_label(&uri, 5);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<script.R#6>"), @r"
        ```breakpoint <script.R#6>
        iteration 3
        ```
        ");
    }

    #[test]
    fn test_format_breakpoint_block_log_error() {
        let uri = test_uri("test.R");
        let result = format_breakpoint_block(&uri, 2, "Error: object 'z' not found\n");
        let link = breakpoint_label(&uri, 2);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<test.R#3>"), @r"
        ```breakpoint <test.R#3>
        Error: object 'z' not found
        ```
        ");
    }

    #[test]
    fn test_format_breakpoint_block_hit_condition_error() {
        let uri = test_uri("test.R");
        let result = format_breakpoint_block(
            &uri,
            17,
            "Error: Expected a positive integer, invalid digit found in string",
        );
        let link = breakpoint_label(&uri, 17);
        insta::assert_snapshot!(result.unwrap().replace(&link, "<test.R#18>"), @r"
        ```breakpoint <test.R#18>
        Error: Expected a positive integer, invalid digit found in string
        ```
        ");
    }

    #[test]
    fn test_breakpoint_label() {
        let uri = test_uri("test.R");
        let link = breakpoint_label(&uri, 4);
        assert_eq!(
            link,
            "\x1b]8;line=5;file:///project/test.R\x07test.R#5\x1b]8;;\x07"
        );
    }
}
