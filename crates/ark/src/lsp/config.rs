use aether_lsp_utils::proto::PositionEncoding;
use biome_line_index::WideEncoding;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use stdext::env_flag_opt;

use crate::lsp::diagnostics::DiagnosticsConfig;

pub struct Setting<T> {
    pub key: &'static str,
    pub set: fn(&mut T, Value),
}

/// Declare global settings with their [`LspSettings`] and [`LspConfig`] fields.
/// Keeping the mapping here synchronizes the request, layering, and resolution
/// code. All global settings are (currently) booleans.
macro_rules! global_settings {
    ($($key:expr => $field:ident : $($config:ident).+),+ $(,)?) => {
        /// A partial settings layer. `None` lets a lower-priority layer or a
        /// default supply the setting.
        #[derive(Clone, Debug, Default, Eq, PartialEq)]
        pub(crate) struct LspSettings {
            $(pub(crate) $field: Option<bool>,)+
        }

        impl LspSettings {
            /// Overlay `self` on `base`, preferring every value set in `self`.
            #[must_use]
            pub(crate) fn or(self, base: Self) -> Self {
                Self {
                    $($field: self.$field.or(base.$field),)+
                }
            }

            /// Apply this layer to `config`. Missing settings use defaults, and
            /// unrelated fields such as the position encoding remain unchanged.
            pub(crate) fn resolve_into(&self, config: &mut LspConfig) {
                $(
                    config.$($config).+ = self
                        .$field
                        .unwrap_or_else(|| LspConfig::default().$($config).+);
                )+
            }
        }

        /// Global settings requested through `workspace/configuration` and
        /// registered for `didChangeConfiguration` notifications.
        pub static GLOBAL_SETTINGS: &[Setting<LspSettings>] = &[
            $(Setting {
                key: $key,
                set: |settings, value| settings.$field = value.as_bool(),
            },)+
        ];
    };
}

// Each row is `key => field: config.path`. It maps an LSP setting key to the
// corresponding fields in [`LspSettings`] and [`LspConfig`].
global_settings! {
    OAK_DIAGNOSTICS_EXPERIMENTAL_ENABLED_SETTING
        => diagnostics_experimental: diagnostics.experimental,
    OAK_SOURCE_FETCHING_ENABLED_SETTING => source_fetching_enabled: oak.source_fetching_enabled,
    "positron.r.diagnostics.enable" => diagnostics_enable: diagnostics.enable,
    "positron.r.symbols.includeAssignmentsInBlocks"
        => include_assignments_in_blocks: symbols.include_assignments_in_blocks,
    "positron.r.workspaceSymbols.includeCommentSections"
        => include_comment_sections: workspace_symbols.include_comment_sections,
}

/// Read global settings from the client's `initializationOptions`.
///
/// Dotted keys from [`GLOBAL_SETTINGS`] follow nested objects. For example,
/// `oak.sourceFetching.enabled` reads `{oak: {sourceFetching: {enabled: ...}}}`.
pub(crate) fn initialization_options(options: &Value) -> LspSettings {
    let mut layer = LspSettings::default();
    for setting in GLOBAL_SETTINGS {
        if let Some(value) = nested_setting(options, setting.key) {
            (setting.set)(&mut layer, value.clone());
        }
    }
    layer
}

fn nested_setting<'options>(options: &'options Value, key: &str) -> Option<&'options Value> {
    let mut value = options;
    for segment in key.split('.') {
        value = value.get(segment)?;
    }
    Some(value)
}

pub(crate) const OAK_DIAGNOSTICS_EXPERIMENTAL_ENABLED_SETTING: &str =
    "oak.diagnostics.experimental.enabled";
pub(crate) const OAK_SOURCE_FETCHING_ENABLED_SETTING: &str = "oak.sourceFetching.enabled";

/// Overrides [`OAK_SOURCE_FETCHING_ENABLED_SETTING`] when set to `1`, `true`,
/// `0`, or `false`. Set to `1` or `true` to enable source fetching on CI.
pub(crate) const OAK_SOURCE_FETCHING_ENABLED_ENV_VAR: &str = "OAK_SOURCE_FETCHING_ENABLED";

pub struct EnvOverride<T> {
    pub name: &'static str,
    pub set: fn(&mut T, bool),
}

/// Environment overrides for LSP settings.
pub static ENV_OVERRIDES: &[EnvOverride<LspSettings>] = &[EnvOverride {
    name: OAK_SOURCE_FETCHING_ENABLED_ENV_VAR,
    set: |settings, on| settings.source_fetching_enabled = Some(on),
}];

/// Read recognized environment values into a settings layer. Unset or invalid
/// values leave their setting available to lower-priority layers.
pub(crate) fn env_settings() -> LspSettings {
    let mut layer = LspSettings::default();
    for env_override in ENV_OVERRIDES {
        if let Some(on) = env_flag_opt(env_override.name) {
            (env_override.set)(&mut layer, on);
        }
    }
    layer
}

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
#[derive(Clone, Debug)]
pub(crate) struct LspConfig {
    pub(crate) diagnostics: DiagnosticsConfig,
    pub(crate) oak: OakConfig,
    pub(crate) symbols: SymbolsConfig,
    pub(crate) workspace_symbols: WorkspaceSymbolsConfig,

    /// Session-wide position encoding for offset <-> LSP-position conversion.
    /// One value for the whole session, not per document. Hard-coded to UTF-16,
    /// the encoding we advertise at `initialize`.
    pub(crate) position_encoding: PositionEncoding,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            diagnostics: DiagnosticsConfig::default(),
            oak: OakConfig::default(),
            symbols: SymbolsConfig::default(),
            workspace_symbols: WorkspaceSymbolsConfig::default(),
            position_encoding: PositionEncoding::Wide(WideEncoding::Utf16),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct OakConfig {
    /// Recover package sources so Oak can analyze dependencies.
    pub source_fetching_enabled: bool,
}

impl Default for OakConfig {
    fn default() -> Self {
        Self {
            source_fetching_enabled: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SymbolsConfig {
    /// Whether to emit assignments in `{` bloks as document symbols.
    pub include_assignments_in_blocks: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
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
