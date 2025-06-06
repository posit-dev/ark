//
// parent_monitor.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

use crossbeam::channel::Sender;

use crate::request::RRequest;

/// No parent monitoring on Windows
pub fn start_parent_monitoring(_r_request_tx: Sender<RRequest>) -> anyhow::Result<()> {
    Ok(())
}
