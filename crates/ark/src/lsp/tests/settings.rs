use oak_db::OakDatabase;
use serde_json::json;

use super::utils::initialize;
use super::utils::initialized;
use super::utils::TestClient;
use crate::lsp::config::initialization_options;
use crate::lsp::config::LEGACY_R_DIAGNOSTICS_ENABLE_SETTING;
use crate::lsp::config::R_DIAGNOSTICS_ENABLED_SETTING;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sources::SourceScheduler;
use crate::lsp::state::WorldState;

#[test]
fn test_legacy_diagnostics_initialization_option_can_disable_new_default() {
    let options = json!({
        "positron": {
            "r": {
                "diagnostics": {
                    "enabled": true,
                    "enable": false
                }
            }
        }
    });

    assert_eq!(
        initialization_options(&options).diagnostics_enable,
        Some(false)
    );
}

#[tokio::test]
async fn test_legacy_diagnostics_setting_can_disable_new_default() {
    let _aux = init_aux_for_test();
    let client = TestClient::new(&[
        (R_DIAGNOSTICS_ENABLED_SETTING, json!(true)),
        (LEGACY_R_DIAGNOSTICS_ENABLE_SETTING, json!(false)),
    ])
    .await;
    let mut state = GlobalState::from_parts(
        client.client(),
        WorldState::new(OakDatabase::new()),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceScheduler::new(None),
        ),
    );
    let workspace = tempfile::tempdir().unwrap();
    let (event, _response_rx) = initialize(workspace.path());

    state.handle_event_to_quiescence(event).await;
    state.handle_event_to_quiescence(initialized()).await;

    assert!(!state.world().config.diagnostics.enable);
}
