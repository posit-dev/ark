//
// methods.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::frontend_comm::DebugSleepParams;
use amalthea::comm::frontend_comm::FrontendFrontendRpcRequest;
use harp::object::RObject;
use libR_shim::SEXP;

use crate::interface::RMain;

#[harp::register]
pub unsafe extern "C" fn ps_frontend_last_active_editor_context() -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let out = main.call_frontend_method(FrontendFrontendRpcRequest::LastActiveEditorContext)?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_frontend_debug_sleep(ms: SEXP) -> anyhow::Result<SEXP> {
    let ms: f64 = RObject::view(ms).try_into()?;
    let params = DebugSleepParams { ms };

    let main = RMain::get();
    let out = main.call_frontend_method(FrontendFrontendRpcRequest::DebugSleep(params))?;
    Ok(out.sexp)
}
