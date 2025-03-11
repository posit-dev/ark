use std::collections::HashSet;

use anyhow::Result;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;

use crate::lsp::completions::parameter_hints::ParameterHints;
use crate::lsp::completions::parameter_hints::{self};
use crate::lsp::completions::sources::composite::call::completions_from_call;
use crate::lsp::completions::sources::composite::document::completions_from_document;
use crate::lsp::completions::sources::composite::find_pipe_root;
use crate::lsp::completions::sources::composite::is_identifier_like;
use crate::lsp::completions::sources::composite::keyword::completions_from_keywords;
use crate::lsp::completions::sources::composite::pipe::completions_from_pipe;
use crate::lsp::completions::sources::composite::search_path::completions_from_search_path;
use crate::lsp::completions::sources::composite::snippets::completions_from_snippets;
use crate::lsp::completions::sources::composite::subset::completions_from_subset;
use crate::lsp::completions::sources::composite::workspace::completions_from_workspace;
use crate::lsp::completions::sources::unique::colon::completions_from_single_colon;
use crate::lsp::completions::sources::unique::comment::completions_from_comment;
use crate::lsp::completions::sources::unique::custom::completions_from_custom_source;
use crate::lsp::completions::sources::unique::extractor::completions_from_at;
use crate::lsp::completions::sources::unique::extractor::completions_from_dollar;
use crate::lsp::completions::sources::unique::namespace::completions_from_namespace;
use crate::lsp::completions::sources::unique::string::completions_from_string;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;
pub(crate) struct CompletionBuilder<'a> {
    context: &'a DocumentContext<'a>,
    state: &'a WorldState,
    parameter_hints: ParameterHints,
}

impl<'a> CompletionBuilder<'a> {
    pub fn new(context: &'a DocumentContext, state: &'a WorldState) -> Self {
        let parameter_hints =
            parameter_hints::parameter_hints(context.node, &context.document.contents);
        Self {
            context,
            state,
            parameter_hints,
        }
    }

    pub fn build(self) -> Result<Vec<CompletionItem>> {
        // Try unique sources first
        if let Some(completions) = self.completions_from_unique_sources()? {
            return Ok(completions);
        }

        // At this point we aren't in a "unique" completion case, so just return a
        // set of reasonable completions based on loaded packages, the open
        // document, the current workspace, and any call related arguments
        self.completions_from_composite_sources()
    }

    pub fn completions_from_unique_sources(&self) -> Result<Option<Vec<CompletionItem>>> {
        // Try to detect a single colon first, which is a special case where we
        // don't provide any completions
        if let Some(completions) = completions_from_single_colon(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_comment(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_string(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_namespace(self.context, self.parameter_hints)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_custom_source(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_dollar(self.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_at(self.context)? {
            return Ok(Some(completions));
        }

        // No unique sources of completions, allow composite sources to run
        Ok(None)
    }

    fn completions_from_composite_sources(&self) -> Result<Vec<CompletionItem>> {
        log::info!("completions_from_composite_sources()");

        let mut completions: Vec<CompletionItem> = vec![];

        let root = find_pipe_root(self.context)?;

        // Try argument completions
        if let Some(mut additional_completions) = completions_from_call(self.context, root.clone())?
        {
            completions.append(&mut additional_completions);
        }

        // Try pipe completions
        if let Some(mut additional_completions) = completions_from_pipe(root.clone())? {
            completions.append(&mut additional_completions);
        }

        // Try subset completions (`[` or `[[`)
        if let Some(mut additional_completions) = completions_from_subset(self.context)? {
            completions.append(&mut additional_completions);
        }

        // Call, pipe, and subset completions should show up no matter what when
        // the user requests completions (this allows them to Tab their way through
        // completions effectively without typing anything). For the rest of the
        // general completions, we require an identifier to begin showing
        // anything.
        if is_identifier_like(self.context.node) {
            completions.append(&mut completions_from_keywords());
            completions.append(&mut completions_from_snippets());
            completions.append(&mut completions_from_search_path(
                self.context,
                self.parameter_hints,
            )?);

            if let Some(mut additional_completions) = completions_from_document(self.context)? {
                completions.append(&mut additional_completions);
            }

            if let Some(mut additional_completions) =
                completions_from_workspace(self.context, self.state, self.parameter_hints)?
            {
                completions.append(&mut additional_completions);
            }
        }

        // Remove duplicates
        let mut uniques = HashSet::new();
        completions.retain(|x| uniques.insert(x.label.clone()));

        // Sort completions by providing custom 'sort' text to be used when
        // ordering completion results. we use some placeholders at the front
        // to 'bin' different completion types differently; e.g. we place parameter
        // completions at the front, followed by variable completions (like pipe
        // completions and subset completions), followed by anything else.
        for item in &mut completions {
            // Start with existing `sort_text` if one exists
            let sort_text = item.sort_text.take();

            let sort_text = match sort_text {
                Some(sort_text) => sort_text,
                None => item.label.clone(),
            };

            case! {
                // Argument name
                item.kind == Some(CompletionItemKind::FIELD) => {
                    item.sort_text = Some(join!["1-", sort_text]);
                }

                // Something like pipe completions, or data frame column names
                item.kind == Some(CompletionItemKind::VARIABLE) => {
                    item.sort_text = Some(join!["2-", sort_text]);
                }

                // Package names generally have higher preference than function
                // names. Particularly useful for `dev|` to get to `devtools::`,
                // as that has a lot of base R functions with similar names.
                item.kind == Some(CompletionItemKind::MODULE) => {
                    item.sort_text = Some(join!["3-", sort_text]);
                }

                => {
                    item.sort_text = Some(join!["4-", sort_text]);
                }
            }
        }

        Ok(completions)
    }
}
