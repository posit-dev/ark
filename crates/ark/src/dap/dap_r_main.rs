//
// dap_r_main.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::cell::RefCell;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::protect::RProtect;
use harp::r_string;
use harp::session::r_sys_calls;
use harp::session::r_sys_frames;
use harp::session::r_sys_functions;
use harp::utils::r_is_null;
use libr::R_NilValue;
use libr::R_Srcref;
use libr::Rf_allocVector;
use libr::Rf_xlength;
use libr::INTSXP;
use libr::SET_INTEGER_ELT;
use libr::SEXP;
use libr::VECTOR_ELT;
use stdext::log_error;

use crate::dap::dap::DapBackendEvent;
use crate::dap::Dap;
use crate::modules::ARK_ENVS;
use crate::thread::RThreadSafe;

pub struct RMainDap {
    /// Underlying dap state
    dap: Arc<Mutex<Dap>>,

    /// Whether or not we are currently in a debugging state.
    debugging: bool,

    /// State inferred from the REPL
    debugger_state: RefCell<DebuggerState>,

    /// The current frame `id`. Unique across all frames within a single debug session.
    /// Reset after `stop_debug()`, not between debug steps. If we reset between steps,
    /// we could potentially have a race condition where `handle_scopes()` could request
    /// a `variables_reference` for a `frame_id` that we've already overwritten the
    /// `variables_reference` for, potentially sending back incorrect information.
    current_frame_info_id: i64,
}

pub struct DebuggerState {
    /// The current call emitted by R as `debug: <call-text>`.
    call_text: DebugCallText,

    /// The last known `start_line` for the active context frame.
    last_start_line: Option<i64>,
}

#[derive(Clone, Debug)]
pub enum DebugCallText {
    None,
    Capturing(String),
    Finalized(String),
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

#[derive(Clone, Debug)]
pub enum FrameSource {
    File(String),
    Text(String),
}

impl RMainDap {
    pub fn new(dap: Arc<Mutex<Dap>>) -> Self {
        Self {
            dap,
            debugging: false,
            debugger_state: RefCell::new(DebuggerState {
                call_text: DebugCallText::None,
                last_start_line: None,
            }),
            current_frame_info_id: 0,
        }
    }

    pub fn is_debugging(&self) -> bool {
        self.debugging
    }

    pub fn start_debug(&mut self, stack: Vec<FrameInfo>) {
        self.debugging = true;
        let mut dap = self.dap.lock().unwrap();
        dap.start_debug(stack)
    }

    pub fn stop_debug(&mut self) {
        let mut dap = self.dap.lock().unwrap();
        dap.stop_debug();
        drop(dap);
        self.reset_frame_id();
        self.debugging = false;
    }

    pub fn handle_stdout(&self, content: &str) {
        // Safety: `handle_stdout()` is only called from `write_console()`
        let mut state = self.debugger_state.borrow_mut();
        let state = state.deref_mut();

        if let DebugCallText::Capturing(ref mut call_text) = state.call_text {
            // Append to current expression if we are currently capturing stdout
            call_text.push_str(content);
            return;
        }

        // `debug: ` is emitted by R (if no srcrefs are available!) right before it emits
        // the current expression we are debugging, so we use that as a signal to begin
        // capturing.
        if content == "debug: " {
            state.call_text = DebugCallText::Capturing(String::new());
            return;
        }

        // Entering or exiting a closure, reset the debug start line state and call text
        if content == "debugging in: " || content == "exiting from: " {
            state.last_start_line = None;
            state.call_text = DebugCallText::None;
            return;
        }
    }

    pub fn finalize_call_text(&self) {
        // Safety: `finalize_call_text()` only called from `read_console()`
        let mut state = self.debugger_state.borrow_mut();
        let state = state.deref_mut();

        match &state.call_text {
            // If not debugging, nothing to do.
            DebugCallText::None => (),
            // If already finalized, keep what we have.
            DebugCallText::Finalized(_) => (),
            // If capturing, transition to finalized.
            DebugCallText::Capturing(call_text) => {
                state.call_text = DebugCallText::Finalized(call_text.clone())
            },
        }
    }

    pub fn send_dap(&self, event: DapBackendEvent) {
        let dap = self.dap.lock().unwrap();
        if let Some(tx) = &dap.backend_events_tx {
            log_error!(tx.send(event));
        }
    }

    pub fn stack_info(&mut self) -> anyhow::Result<Vec<FrameInfo>> {
        // We leave finalized `call_text` in place rather than setting it to `None` here
        // in case the user executes an arbitrary expression in the debug R console, which
        // loops us back here without updating the `call_text` in any way, allowing us to
        // recreate the debugger state after their code execution.
        let call_text = match self.debugger_state.borrow().call_text.clone() {
            DebugCallText::None => None,
            DebugCallText::Capturing(call_text) => {
                log::error!(
                    "Call text is in `Capturing` state, but should be `Finalized`: '{call_text}'."
                );
                None
            },
            DebugCallText::Finalized(call_text) => Some(call_text),
        };

        let last_start_line = self.debugger_state.borrow().last_start_line;
        let frames = self.r_stack_info(call_text, last_start_line)?;

        // If we have `frames`, update the `last_start_line` with the context
        // frame's start line
        if let Some(frame) = frames.get(0) {
            // Safety: `stack_info()` only called from `read_console()`
            let mut state = self.debugger_state.borrow_mut();
            let state = state.deref_mut();

            state.last_start_line = Some(frame.start_line);
        }

        Ok(frames)
    }

    fn r_stack_info(
        &mut self,
        context_call_text: Option<String>,
        context_last_start_line: Option<i64>,
    ) -> anyhow::Result<Vec<FrameInfo>> {
        unsafe {
            let mut protect = RProtect::new();

            let context_srcref = libr::get(R_Srcref);
            protect.add(context_srcref);

            let context_call_text = match context_call_text {
                Some(context_call_text) => r_string!(context_call_text, &mut protect),
                None => R_NilValue,
            };

            let context_last_start_line = match context_last_start_line {
                Some(context_last_start_line) => {
                    let x = Rf_allocVector(INTSXP, 1);
                    protect.add(x);
                    SET_INTEGER_ELT(x, 0, i32::try_from(context_last_start_line)?);
                    x
                },
                None => R_NilValue,
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

            let n: isize = Rf_xlength(info.sexp);

            let mut out = Vec::with_capacity(n as usize);

            // Reverse the order for DAP
            for i in (0..n).rev() {
                let frame = VECTOR_ELT(info.sexp, i);
                out.push(self.as_frame_info(frame)?);
            }

            Ok(out)
        }
    }

    fn as_frame_info(&mut self, info: SEXP) -> anyhow::Result<FrameInfo> {
        unsafe {
            let mut i = 0;

            let source_name = VECTOR_ELT(info, i);
            let source_name: String = RObject::view(source_name).try_into()?;

            i += 1;
            let frame_name = VECTOR_ELT(info, i);
            let frame_name: String = RObject::view(frame_name).try_into()?;

            let mut source = None;

            i += 1;
            let file = VECTOR_ELT(info, i);
            if file != R_NilValue {
                let file: String = RObject::view(file).try_into()?;
                source = Some(FrameSource::File(file));
            }

            i += 1;
            let text = VECTOR_ELT(info, i);
            if text != R_NilValue {
                let text: String = RObject::view(text).try_into()?;
                source = Some(FrameSource::Text(text));
            }

            let Some(source) = source else {
                return Err(anyhow!(
                    "Expected either `file` or `text` to be non-`NULL`."
                ));
            };

            i += 1;
            let environment = VECTOR_ELT(info, i);
            let environment = if r_is_null(environment) {
                None
            } else {
                Some(RThreadSafe::new(RObject::from(environment)))
            };

            i += 1;
            let start_line = VECTOR_ELT(info, i);
            let start_line: i32 = RObject::view(start_line).try_into()?;

            i += 1;
            let start_column = VECTOR_ELT(info, i);
            let start_column: i32 = RObject::view(start_column).try_into()?;

            i += 1;
            let end_line = VECTOR_ELT(info, i);
            let end_line: i32 = RObject::view(end_line).try_into()?;

            // For `end_column`, the column range provided by R is inclusive `[,]`, but the
            // one used on the DAP / Positron side is exclusive `[,)` so we have to add 1.
            i += 1;
            let end_column = VECTOR_ELT(info, i);
            let end_column: i32 = RObject::view(end_column).try_into()?;
            let end_column = end_column + 1;

            let id = self.next_frame_id();

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

    fn next_frame_id(&mut self) -> i64 {
        let out = self.current_frame_info_id;
        self.current_frame_info_id += 1;
        out
    }

    fn reset_frame_id(&mut self) {
        self.current_frame_info_id = 0;
    }
}
