//
// request.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::Position;

use crate::lsp::call_arguments::CallArguments;
use crate::lsp::completions::function_context::FunctionContext;

/// Owned input for session-dependent completion sources recognized by the
/// static pass.
///
/// Contains no database handles, syntax trees, or R objects, so it can be
/// sent to the R thread. Each `Option` is a source's slot, `None` when that
/// source doesn't apply, and `composite` holds the composite sources' slots
/// the same way. Custom callbacks take priority
/// over the unique and composite sources when they return an override.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct RCompletionRequest {
    /// Input for custom callbacks, as in `Sys.getenv(|)`, with `|` marking the
    /// cursor. Only the session can determine whether a callback applies, so
    /// the static pass also collects fallback items.
    pub(crate) custom: Option<CustomRequest>,

    /// The session-dependent unique source that ended the static search.
    /// Its static fallback is empty.
    pub(crate) unique: Option<UniqueRequest>,

    /// Session-dependent composite sources. Empty when a unique source ended
    /// the static search.
    pub(crate) composite: CompositeRequest,
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

/// Inputs for session-dependent composite sources. Their replies are merged
/// with static items in source order.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct CompositeRequest {
    /// Text of the root of the pipe containing the cursor's call, as in `df`
    /// for `df |> f(|)`, where `|` marks the cursor. Requests names of the
    /// evaluated root for the pipe slot. The call slot shares that evaluation
    /// for S3 dispatch.
    pub(crate) pipe_root: Option<String>,

    /// Argument names of the call under the cursor.
    pub(crate) call: Option<CallRequest>,

    /// Names of the evaluated `object` in `x[|]` or `x[[|]]`.
    pub(crate) subset: Option<SubsetRequest>,

    /// Symbols on the search path from the selected environment, and installed
    /// packages.
    pub(crate) search_path: Option<SearchPathRequest>,
}

/// Argument names of `callee` when the cursor is in an argument name
/// position. Session formals replace the workspace formals in the static
/// call slot. If the callee cannot be evaluated, the workspace formals remain.
#[derive(Debug, PartialEq)]
pub(crate) struct CallRequest {
    pub(crate) callee: String,
    pub(crate) dispatch: CallDispatch,
    /// Cursor position for the `...` item's empty edit, which shows
    /// documentation without inserting text.
    pub(crate) position: Position,
}

/// The object passed to `.ps.completions.formalNames()` for S3 dispatch.
#[derive(Debug, PartialEq)]
pub(crate) enum CallDispatch {
    /// Use the evaluated pipe root, or `NULL` if evaluation fails, without
    /// falling back to the call's first argument.
    PipeRoot,
    /// Evaluate the unnamed first argument, using `NULL` if evaluation fails.
    FirstArgument(String),
    /// `NULL`.
    None,
}

#[derive(Debug, PartialEq)]
pub(crate) struct SubsetRequest {
    pub(crate) object: String,
    pub(crate) operator: SubsetOperator,
}

#[derive(Debug, PartialEq)]
pub(crate) struct SearchPathRequest {
    pub(crate) function_context: FunctionContext,
    /// Whether the token under the cursor starts with `.`. Otherwise names
    /// starting with `.` are left out.
    pub(crate) dot_prefixed: bool,
}

/// The session pass's answer for each slot of an [RCompletionRequest].
#[derive(Debug, Default)]
pub(crate) struct RCompletionReplies {
    pub(crate) custom: SlotReply,
    pub(crate) unique: SlotReply,
    pub(crate) composite: CompositeReplies,
}

#[derive(Debug, Default)]
pub(crate) struct CompositeReplies {
    pub(crate) call: SlotReply,
    pub(crate) pipe: SlotReply,
    pub(crate) subset: SlotReply,
    pub(crate) search_path: SlotReply,
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

impl SlotReply {
    /// Returns `fallback` for [SlotReply::Decline], otherwise the reply's items.
    /// Does not apply the whole-list semantics of [SlotReply::Override].
    pub(crate) fn items_or(self, fallback: Vec<CompletionItem>) -> Vec<CompletionItem> {
        debug_assert!(
            !matches!(self, SlotReply::Override(_)),
            "Only the custom slot can override"
        );
        match self {
            SlotReply::Decline => fallback,
            SlotReply::Fill(items) | SlotReply::Override(items) => items,
        }
    }
}
