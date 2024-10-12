//
// events.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

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
    main.ui_send_event(event);
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
    main.ui_send_event(event);
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
    main.ui_send_event(event);
    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_set_selection_ranges(ranges: SEXP) -> anyhow::Result<SEXP> {
    let selections = ps_ui_robj_as_ranges(ranges)?;

    let params = SetEditorSelectionsParams { selections };

    let main = RMain::get();
    let event = UiFrontendEvent::SetEditorSelections(params);
    main.ui_send_event(event);
    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_ui_show_url(url: SEXP) -> anyhow::Result<SEXP> {
    let params = ShowUrlParams {
        url: RObject::view(url).try_into()?,
    };

    let main = RMain::get();
    let event = UiFrontendEvent::ShowUrl(params);
    main.ui_send_event(event);
    Ok(R_NilValue)
}

pub fn ps_ui_robj_as_ranges(ranges: SEXP) -> anyhow::Result<Vec<Range>> {
    let ranges_as_r_objects: Vec<RObject> = RObject::view(ranges).try_into()?;
    let ranges_as_result: Result<Vec<Vec<i32>>, _> = ranges_as_r_objects
        .iter()
        .map(|x| Vec::<i32>::try_from(x.clone()))
        .collect();
    let ranges_as_vec_of_vecs = ranges_as_result?;
    let selections: Vec<Range> = ranges_as_vec_of_vecs
        .iter()
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
    Ok(selections)
}
