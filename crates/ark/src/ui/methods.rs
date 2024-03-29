//
// methods.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::ui_comm::DebugSleepParams;
use amalthea::comm::ui_comm::ModifyEditorSelectionsParams;
use amalthea::comm::ui_comm::ShowDialogParams;
use amalthea::comm::ui_comm::ShowQuestionParams;
use amalthea::comm::ui_comm::UiFrontendRequest;
use harp::object::RObject;
use harp::utils::r_is_null;
use libr::SEXP;

use crate::interface::RMain;
use crate::ui::events::ps_ui_robj_as_ranges;

#[harp::register]
pub unsafe extern "C" fn ps_ui_last_active_editor_context() -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::LastActiveEditorContext)?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_modify_editor_selections(
    ranges: SEXP,
    values: SEXP,
) -> anyhow::Result<SEXP> {
    let selections = ps_ui_robj_as_ranges(ranges)?;
    let values: Vec<String> = RObject::view(values).try_into()?;
    if selections.len() != values.len() {
        log::error!(
            "Error modifying editor selections: selections and values are not the same length."
        );
    }
    let params = ModifyEditorSelectionsParams { selections, values };

    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::ModifyEditorSelections(params))?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_workspace_folder() -> anyhow::Result<SEXP> {
    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::WorkspaceFolder)?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_show_dialog(title: SEXP, message: SEXP) -> anyhow::Result<SEXP> {
    let params = ShowDialogParams {
        title: RObject::view(title).try_into()?,
        message: RObject::view(message).try_into()?,
    };

    let main = RMain::get();
    let out = main.call_frontend_method(UiFrontendRequest::ShowDialog(params))?;
    Ok(out.sexp)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_show_question(
    title: SEXP,
    message: SEXP,
    ok_button_title: SEXP,
    cancel_button_title: SEXP,
) -> anyhow::Result<SEXP> {
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
