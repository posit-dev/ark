//
// events.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use amalthea::comm::ui_comm::ExecuteCommandParams;
use amalthea::comm::ui_comm::OpenEditorParams;
use amalthea::comm::ui_comm::OpenWorkspaceParams;
use amalthea::comm::ui_comm::Position;
use amalthea::comm::ui_comm::Range;
use amalthea::comm::ui_comm::SetEditorSelectionsParams;
use amalthea::comm::ui_comm::ShowMessageParams;
use amalthea::comm::ui_comm::ShowUrlParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;

use crate::interface::RMain;

#[harp::register]
pub unsafe extern "C" fn ps_ui_show_message(message: SEXP) -> anyhow::Result<SEXP> {
    let params = ShowMessageParams {
        message: RObject::view(message).try_into()?,
    };

    let main = RMain::get();
    let event = UiFrontendEvent::ShowMessage(params);
    main.send_frontend_event(event);
    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_execute_command(command: SEXP) -> anyhow::Result<SEXP> {
    let params = ExecuteCommandParams {
        command: RObject::view(command).try_into()?,
    };

    let main = RMain::get();
    let event = UiFrontendEvent::ExecuteCommand(params);
    main.send_frontend_event(event);
    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_open_workspace(
    path: SEXP,
    new_window: SEXP,
) -> anyhow::Result<SEXP> {
    let params = OpenWorkspaceParams {
        path: RObject::view(path).try_into()?,
        new_window: RObject::view(new_window).try_into()?,
    };

    let main = RMain::get();
    let event = UiFrontendEvent::OpenWorkspace(params);
    main.send_frontend_event(event);
    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_navigate_to_file(
    file: SEXP,
    _line: SEXP,
    _column: SEXP,
) -> anyhow::Result<SEXP> {
    let params = OpenEditorParams {
        file: RObject::view(file).try_into()?,
        line: 0,
        column: 0,
    };

    let main = RMain::get();
    let event = UiFrontendEvent::OpenEditor(params);
    main.send_frontend_event(event);
    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_set_selection_ranges(ranges: SEXP) -> anyhow::Result<SEXP> {
    let ranges_smushed_together: Vec<i32> = RObject::view(ranges).try_into()?;
    let ranges: Vec<Range> = ranges_smushed_together
        .chunks_exact(4)
        .map(|chunk| Range {
            start: Position {
                character: chunk[1] as i64,
                line: chunk[0] as i64,
            },
            end: Position {
                character: chunk[3] as i64,
                line: chunk[2] as i64,
            },
        })
        .collect();

    let params = SetEditorSelectionsParams { selections: ranges };

    let main = RMain::get();
    let event = UiFrontendEvent::SetEditorSelections(params);
    main.send_frontend_event(event);
    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_show_url(url: SEXP) -> anyhow::Result<SEXP> {
    let params = ShowUrlParams {
        url: RObject::view(url).try_into()?,
    };

    let main = RMain::get();
    let event = UiFrontendEvent::ShowUrl(params);
    main.send_frontend_event(event);
    Ok(R_NilValue)
}
