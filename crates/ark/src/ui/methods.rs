//
// methods.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::ui_comm::DebugSleepParams;
use amalthea::comm::ui_comm::DocumentNewParams;
use amalthea::comm::ui_comm::Position;
use amalthea::comm::ui_comm::ShowQuestionParams;
use amalthea::comm::ui_comm::UiFrontendRequest;
use harp::object::RObject;
use harp::utils::r_is_null;
use libr::SEXP;

use crate::interface::RMain;

#[harp::register]
pub unsafe extern "C" fn ps_ui_last_active_editor_context() -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::LastActiveEditorContext)?;
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
pub unsafe extern "C" fn ps_ui_show_question(
    title: SEXP,
    message: SEXP,
    ok_button_title: SEXP,
    cancel_button_title: SEXP,
) -> anyhow::Result<SEXP> {
    if !r_is_null(ok_button_title) {}

    let params = ShowQuestionParams {
        title: RObject::view(title).try_into()?,
        message: RObject::view(message).try_into()?,
        ok_button_title: if r_is_null(ok_button_title) {
            String::from("OK")
        } else {
            RObject::view(ok_button_title).try_into()?
        },
        cancel_button_title: if r_is_null(cancel_button_title) {
            String::from("Cancel")
        } else {
            RObject::view(cancel_button_title).try_into()?
        },
    };

    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::ShowQuestion(params))?;
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
