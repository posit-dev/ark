use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::lsp::diagnostics::DiagnosticsConfig;

pub struct Setting<T> {
    pub key: &'static str,
    pub set: fn(&mut T, Value),
}

/// List of LSP settings for which clients can send `didChangeConfiguration`
/// notifications. We register our interest in watching over these settings in
/// our `initialized` handler. The `set` methods convert from a json `Value` to
/// the expected type, using a default value if the conversion fails.
///
/// This array is for global settings. If the setting should only affect a given
/// document URI, add it to `DOCUMENT_SETTINGS` instead.
pub static GLOBAL_SETTINGS: &[Setting<LspConfig>] = &[
    Setting {
        key: "positron.r.diagnostics.enable",
        set: |cfg, v| {
            cfg.diagnostics.enable = v
                .as_bool()
                .unwrap_or_else(|| DiagnosticsConfig::default().enable)
        },
    },
    Setting {
        key: "positron.r.symbols.includeAssignmentsInBlocks",
        set: |cfg, v| {
            cfg.symbols.include_assignments_in_blocks = v
                .as_bool()
                .unwrap_or_else(|| SymbolsConfig::default().include_assignments_in_blocks)
        },
    },
    Setting {
        key: "positron.r.workspaceSymbols.includeCommentSections",
        set: |cfg, v| {
            cfg.workspace_symbols.include_comment_sections = v
                .as_bool()
                .unwrap_or_else(|| WorkspaceSymbolsConfig::default().include_comment_sections)
        },
    },
];

/// These document settings are updated on a URI basis. Each document has its
/// own value of the setting.
pub static DOCUMENT_SETTINGS: &[Setting<DocumentConfig>] = &[
    Setting {
        key: "editor.insertSpaces",
        set: |cfg, v| {
            let default_style = IndentationConfig::default().indent_style;
            cfg.indent.indent_style = if v
                .as_bool()
                .unwrap_or_else(|| default_style == IndentStyle::Space)
            {
                IndentStyle::Space
            } else {
                IndentStyle::Tab
            }
        },
    },
    Setting {
        key: "editor.indentSize",
        set: |cfg, v| {
            cfg.indent.indent_size = v
                .as_u64()
                .map(|n| n as usize)
                .unwrap_or_else(|| IndentationConfig::default().indent_size)
        },
    },
    Setting {
        key: "editor.tabSize",
        set: |cfg, v| {
            cfg.indent.tab_width = v
                .as_u64()
                .map(|n| n as usize)
                .unwrap_or_else(|| IndentationConfig::default().tab_width)
        },
    },
];

/// Configuration of the LSP
#[derive(Clone, Default, Debug)]
pub(crate) struct LspConfig {
    pub(crate) diagnostics: DiagnosticsConfig,
    pub(crate) symbols: SymbolsConfig,
    pub(crate) workspace_symbols: WorkspaceSymbolsConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SymbolsConfig {
    /// Whether to emit assignments in `{` bloks as document symbols.
    pub include_assignments_in_blocks: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WorkspaceSymbolsConfig {
    /// Whether to include sections like `# My section ---` in workspace symbols.
    pub include_comment_sections: bool,
}

/// Configuration of a document.
///
/// The naming follows <https://editorconfig.org/> where possible.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct DocumentConfig {
    pub indent: IndentationConfig,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IndentationConfig {
    /// Whether to insert spaces of tabs for one level of indentation.
    pub indent_style: IndentStyle,

    /// The number of spaces for one level of indentation.
    pub indent_size: usize,

    /// The width of a tab. There may be projects with an `indent_size` of 4 and
    /// a `tab_width` of 8 (e.g. GNU R).
    pub tab_width: usize,
}

#[derive(PartialEq, Serialize, Deserialize, Clone, Debug)]
pub enum IndentStyle {
    Tab,
    Space,
}

impl Default for SymbolsConfig {
    fn default() -> Self {
        Self {
            include_assignments_in_blocks: false,
        }
    }
}

impl Default for WorkspaceSymbolsConfig {
    fn default() -> Self {
        Self {
            include_comment_sections: false,
        }
    }
}

impl Default for IndentationConfig {
    fn default() -> Self {
        Self {
            indent_style: IndentStyle::Space,
            indent_size: 2,
            tab_width: 2,
        }
    }
}

pub(crate) fn indent_style_from_lsp(insert_spaces: bool) -> IndentStyle {
    if insert_spaces {
        IndentStyle::Space
    } else {
        IndentStyle::Tab
    }
}
