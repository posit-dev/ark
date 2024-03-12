//
// methods.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::ui_comm::DebugSleepParams;
use amalthea::comm::ui_comm::DocumentNewParams;
use amalthea::comm::ui_comm::ExecuteCommandParams;
use amalthea::comm::ui_comm::NavigateToFileParams;
use amalthea::comm::ui_comm::Position;
use amalthea::comm::ui_comm::UiFrontendRequest;
use harp::object::RObject;
use libr::SEXP;

use crate::interface::RMain;

#[harp::register]
pub unsafe extern "C" fn ps_ui_last_active_editor_context() -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::LastActiveEditorContext)?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_execute_command(command: SEXP) -> anyhow::Result<SEXP> {
    let params = ExecuteCommandParams {
        command: RObject::view(command).try_into()?,
    };

    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::ExecuteCommand(params))?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_document_new(
    contents: SEXP,
    language_id: SEXP,
    _character: SEXP,
    _line: SEXP,
) -> anyhow::Result<SEXP> {
    let params = DocumentNewParams {
        contents: RObject::view(contents).try_into()?,
        language_id: RObject::view(language_id).try_into()?,
        position: Position {
            character: 0,
            line: 0,
        },
    };

    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::DocumentNew(params))?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_navigate_to_file(file: SEXP) -> anyhow::Result<SEXP> {
    let params = NavigateToFileParams {
        file: RObject::view(file).try_into()?,
    };

    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::NavigateToFile(params))?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_debug_sleep(ms: SEXP) -> anyhow::Result<SEXP> {
    let params = DebugSleepParams {
        ms: RObject::view(ms).try_into()?,
    };

    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::DebugSleep(params))?;
    Ok(out.sexp)
}
