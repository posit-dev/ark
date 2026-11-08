//
// request.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::call_arguments::CallArguments;
use crate::lsp::completions::function_context::FunctionContext;

/// Owned input for session-dependent completion sources recognized by the
/// static pass.
///
/// Contains no database handles, syntax trees, or R objects, so it can be
/// sent to the R thread. Each field represents a source's reply slot, and
/// `None` means that source does not apply. Custom callbacks take priority
/// over the unique source when they return an override.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct RCompletionRequest {
    /// Input for custom callbacks, as in `Sys.getenv(|)`, with `|` marking the
    /// cursor. Only the session can determine whether a callback applies, so
    /// the static pass also collects fallback items.
    pub(crate) custom: Option<CustomRequest>,

    /// The session-dependent unique source that ended the static search.
    /// Its static fallback is empty.
    pub(crate) unique: Option<UniqueRequest>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct CustomRequest {
    pub(crate) arguments: CallArguments,
    pub(crate) position: ArgumentPosition,
    /// Whether the cursor is on a string node, so callbacks requesting quoted
    /// values do not add another pair of quotes.
    pub(crate) in_string: bool,
}

/// Where the cursor is in `fn(name = value)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ArgumentPosition {
    Name,
    Value,
}

#[derive(Debug, PartialEq)]
pub(crate) enum UniqueRequest {
    /// Names of the evaluated `object` in `x["|"]` or `x[c("a", "|")]`.
    StringSubset { object: String },

    /// File path completions for a string outside recognized string subsetting.
    /// `token` includes quotes and escapes as written in the document.
    FilePath { token: String },

    /// `pkg::|` or `pkg:::|`. `exports_only` is true for `::`.
    Namespace {
        package: String,
        exports_only: bool,
        function_context: FunctionContext,
    },

    /// Names of the evaluated `object` after `$` or `@`.
    Extractor {
        object: String,
        operator: ExtractOperator,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ExtractOperator {
    Dollar,
    At,
}

/// Whether a subset is `x[` or `x[[`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SubsetOperator {
    Single,
    Double,
}

/// The session pass's answer for each slot of an [RCompletionRequest].
#[derive(Debug, Default)]
pub(crate) struct RCompletionReplies {
    pub(crate) custom: SlotReply,
    pub(crate) unique: SlotReply,
}

#[derive(Debug, Default, PartialEq)]
pub(crate) enum SlotReply {
    /// Keeps the static fallback. Used when no session reply is available,
    /// the source does not apply, or the source returns an error.
    #[default]
    Decline,

    /// Replaces the slot's static items with the session's items.
    Fill(Vec<CompletionItem>),

    /// Replaces the entire completion list, even when the reply is empty.
    /// Only the custom source returns this reply.
    Override(Vec<CompletionItem>),
}
