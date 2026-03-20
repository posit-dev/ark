//
// events.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::str::FromStr;

use amalthea::comm::ui_comm::OpenEditorKind;
use amalthea::comm::ui_comm::OpenEditorParams;
use amalthea::comm::ui_comm::OpenWithSystemParams;
use amalthea::comm::ui_comm::OpenWorkspaceParams;
use amalthea::comm::ui_comm::Position;
use amalthea::comm::ui_comm::Range;
use amalthea::comm::ui_comm::SetEditorSelectionsParams;
use amalthea::comm::ui_comm::ShowUrlParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use harp::object::RObject;
use libr::R_NilValue;
use libr::SEXP;

use crate::console::Console;

#[harp::register]
pub unsafe extern "C-unwind" fn ps_ui_show_message(message: SEXP) -> anyhow::Result<SEXP> {
    let message: String = RObject::view(message).try_into()?;

    Console::get().try_ui_comm()?.show_message(message);

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_ui_open_workspace(
    path: SEXP,
    new_window: SEXP,
) -> anyhow::Result<SEXP> {
    let params = OpenWorkspaceParams {
        path: RObject::view(path).try_into()?,
        new_window: RObject::view(new_window).try_into()?,
    };

    let event = UiFrontendEvent::OpenWorkspace(params);

    Console::get().try_ui_comm()?.send_event(&event);

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_ui_navigate_to_file(
    file: SEXP,
    line: SEXP,
    column: SEXP,
    uri: SEXP,
) -> anyhow::Result<SEXP> {
    let kind: String = RObject::view(uri).try_into()?;
    let kind = OpenEditorKind::from_str(&kind)?;

    let params = OpenEditorParams {
        file: RObject::view(file).try_into()?,
        line: RObject::view(line).try_into()?,
        column: RObject::view(column).try_into()?,
        kind,
        pinned: None,
    };

    let event = UiFrontendEvent::OpenEditor(params);

    Console::get().try_ui_comm()?.send_event(&event);

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_ui_set_selection_ranges(ranges: SEXP) -> anyhow::Result<SEXP> {
    let selections = ps_ui_robj_as_ranges(ranges)?;
    let params = SetEditorSelectionsParams { selections };

    let event = UiFrontendEvent::SetEditorSelections(params);

    Console::get().try_ui_comm()?.send_event(&event);

    Ok(R_NilValue)
}

pub fn send_show_url_event(url: &str) -> anyhow::Result<()> {
    let params = ShowUrlParams {
        url: url.to_string(),
        source: None,
    };
    let event = UiFrontendEvent::ShowUrl(params);

    Console::get().try_ui_comm()?.send_event(&event);

    Ok(())
}

#[harp::register]
pub unsafe extern "C-unwind" fn ps_ui_show_url(url: SEXP) -> anyhow::Result<SEXP> {
    let url_string = RObject::view(url).to::<String>()?;
    send_show_url_event(&url_string)?;
    Ok(R_NilValue)
}

pub fn send_open_with_system_event(path: &str) -> anyhow::Result<()> {
    let params = OpenWithSystemParams {
        path: path.to_string(),
    };
    let event = UiFrontendEvent::OpenWithSystem(params);

    Console::get().try_ui_comm()?.send_event(&event);

    Ok(())
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
