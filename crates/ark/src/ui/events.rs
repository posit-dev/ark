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
use amalthea::comm::ui_comm::SetCursorPositionParams;
use amalthea::comm::ui_comm::ShowMessageParams;
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
pub unsafe extern "C" fn ps_ui_set_cursor_position(
    character: SEXP,
    line: SEXP,
) -> anyhow::Result<SEXP> {
    let character: i32 = RObject::view(character).try_into()?;
    let line: i32 = RObject::view(line).try_into()?;

    let params = SetCursorPositionParams {
        position: Position {
            character: character as i64,
            line: line as i64,
        },
    };

    let main = RMain::get();
    let event = UiFrontendEvent::SetCursorPosition(params);
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
