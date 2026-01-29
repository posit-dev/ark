use zed_extension_api::{self as zed, LanguageServerId, Result};

struct ArkLspExtension;

impl zed::Extension for ArkLspExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        // Check for ark-lsp in PATH or common locations
        let path = worktree
            .which("ark-lsp")
            .ok_or_else(|| "ark-lsp not found in PATH. Install from https://github.com/posit-dev/ark".to_string())?;

        Ok(zed::Command {
            command: path,
            args: vec!["--stdio".to_string()],
            env: Default::default(),
        })
    }
}

zed::register_extension!(ArkLspExtension);
