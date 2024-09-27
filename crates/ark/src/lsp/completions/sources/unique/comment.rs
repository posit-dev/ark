//
// comment.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::path::Path;

use anyhow::Result;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use regex::Regex;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use yaml_rust::YamlLoader;

use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NodeTypeExt;

pub fn completions_from_comment(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_comment()");

    let node = context.node;

    if !node.is_comment() {
        return Ok(None);
    }

    let pattern = Regex::new(r"^.*\s")?;

    let contents = context.document.contents.node_slice(&node)?.to_string();
    let token = pattern.replace(contents.as_str(), "");

    let mut completions: Vec<CompletionItem> = vec![];

    if !token.starts_with('@') {
        // We are done, there are no completions, but we are in a comment so
        // no one else should get a chance to register anything
        return Ok(Some(completions));
    }

    // TODO: cache these?
    // TODO: use an indexer to build the tag list?
    let tags = unsafe {
        RFunction::new("base", "system.file")
            .param("package", "roxygen2")
            .add("roxygen2-tags.yml")
            .call()?
            .to::<String>()?
    };

    if tags.is_empty() {
        return Ok(Some(completions));
    }

    let tags = Path::new(&tags);
    if !tags.exists() {
        return Ok(Some(completions));
    }

    let contents = std::fs::read_to_string(tags).unwrap();
    let docs = YamlLoader::load_from_str(contents.as_str()).unwrap();
    let doc = &docs[0];

    let items = doc.as_vec().unwrap();
    for entry in items.iter() {
        let Some(name) = entry["name"].as_str() else {
            continue;
        };

        let template = entry["template"].as_str();
        let template = template.map(inject_roxygen_comment_after_newline);
        let template = template.as_deref();

        let description = entry["description"].as_str();

        let item = completion_item_from_roxygen(name, template, description)?;

        completions.push(item);
    }

    Ok(Some(completions))
}

fn completion_item_from_roxygen(
    name: &str,
    template: Option<&str>,
    description: Option<&str>,
) -> Result<CompletionItem> {
    let label = name.to_string();

    let mut item = completion_item(label.clone(), CompletionData::RoxygenTag {
        tag: label.clone(),
    })?;

    // TODO: What is the appropriate icon for us to use here?
    if let Some(template) = template {
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        item.insert_text = Some(format!("{name}{template}"));
    } else {
        item.insert_text = Some(format!("{label}"));
    }

    item.detail = Some(format!("roxygen @{} (R)", name));
    if let Some(description) = description {
        let markup = MarkupContent {
            kind: MarkupKind::Markdown,
            value: description.to_string(),
        };
        item.documentation = Some(Documentation::MarkupContent(markup));
    }

    Ok(item)
}

fn inject_roxygen_comment_after_newline(x: &str) -> String {
    x.replace("\n", "\n#' ")
}

#[test]
fn test_comment() {
    use tree_sitter::Point;

    use crate::r_task;
    use crate::lsp::documents::Document;

    r_task(|| {
        // If not in a comment, return `None`
        let point = Point { row: 0, column: 1 };
        let document = Document::new("mean()", None);
        let context = DocumentContext::new(&document, point, None);
        let completions = completions_from_comment(&context).unwrap();
        assert!(completions.is_none());

        // If in a comment, return empty vector
        let point = Point { row: 0, column: 1 };
        let document = Document::new("# mean", None);
        let context = DocumentContext::new(&document, point, None);
        let completions = completions_from_comment(&context).unwrap().unwrap();
        assert!(completions.is_empty());
    });
}

#[test]
fn test_roxygen_comment() {
    use libr::LOGICAL_ELT;
    use tree_sitter::Point;

    use crate::r_task;
    use crate::lsp::documents::Document;

    r_task(|| unsafe {
        let installed = RFunction::new("", ".ps.is_installed")
            .add("roxygen2")
            .add("7.2.1.9000")
            .call()
            .unwrap();
        let installed = LOGICAL_ELT(*installed, 0) != 0;

        if !installed {
            return;
        }

        let point = Point { row: 0, column: 4 };
        let document = Document::new("#' @", None);
        let context = DocumentContext::new(&document, point, None);
        let completions = completions_from_comment(&context).unwrap().unwrap();

        // Make sure we find it
        let aliases: Vec<&CompletionItem> = completions
            .iter()
            .filter(|item| item.label == "aliases")
            .collect();
        assert_eq!(aliases.len(), 1);

        // Replace `\n` with `\n#' ` since we are directly injecting into the
        // document with no allowance for context specific rules to kick in
        // and automatically add the leading comment for us.
        let description: Vec<&CompletionItem> = completions
            .iter()
            .filter(|item| item.label == "description")
            .collect();
        let description = description.get(0).unwrap();
        assert_eq!(
            description.insert_text,
            Some(String::from(
                "description\n#' ${1:A short description...}\n#' "
            ))
        );
    });
}

#[test]
fn test_roxygen_completion_item() {
    let name = "aliases";
    let template = " ${1:alias}";
    let description = "Add additional aliases to the topic.";

    // With all optional details
    let item = completion_item_from_roxygen(name, Some(template), Some(description)).unwrap();
    assert_eq!(item.label, name);
    assert_eq!(item.detail, Some("roxygen @aliases (R)".to_string()));
    assert_eq!(item.insert_text, Some("aliases ${1:alias}".to_string()));

    let markup = Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value: description.to_string(),
    });
    assert_eq!(item.documentation, Some(markup));

    // Without optional details
    let name = "export";
    let item = completion_item_from_roxygen(name, None, None).unwrap();
    assert_eq!(item.label, name);
    assert_eq!(item.insert_text, Some("export".to_string()));
    assert_eq!(item.documentation, None);
}
