//
// comment.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use std::sync::LazyLock;

use oak_semantic::library::Library;
use regex::Regex;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::InsertTextFormat;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;
use yaml_rust2::YamlLoader;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::completions::types::CompletionData;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeTypeExt;

pub(super) struct CommentSource;

impl CompletionSource for CommentSource {
    fn name(&self) -> &'static str {
        "comment"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_comment(
            completion_context.document_context,
            &completion_context.state.library,
        )
    }
}

// Strip everything up to and including the last whitespace
static RE_UP_TO_LAST_WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.*\s").unwrap());

fn completions_from_comment(
    context: &DocumentContext,
    library: &Library,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    let node = context.node;

    if !node.is_comment() {
        return Ok(None);
    }

    let contents = node.node_as_str(&context.document.contents)?;
    let token = RE_UP_TO_LAST_WHITESPACE.replace(contents, "");

    let mut completions: Vec<CompletionItem> = vec![];

    if !token.starts_with('@') {
        // We are done, there are no completions, but we are in a comment so
        // no one else should get a chance to register anything
        return Ok(Some(completions));
    }

    let Some(roxygen2) = library.get("roxygen2") else {
        return Ok(Some(completions));
    };

    let tags = roxygen2.path().join("roxygen2-tags.yml");

    if !tags.exists() {
        return Ok(Some(completions));
    }

    // TODO: Cache these?
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
) -> anyhow::Result<CompletionItem> {
    let label = name.to_string();

    let mut item = completion_item(label.clone(), CompletionData::RoxygenTag {
        tag: label.clone(),
    })?;

    // TODO: What is the appropriate icon for us to use here?
    if let Some(template) = template {
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
        item.insert_text = Some(format!("{name}{template}"));
    } else {
        item.insert_text = Some(label.to_string());
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

    use crate::lsp::document::Document;

    let library = Library::new(vec![], None);

    // If not in a comment, return `None`
    let point = Point { row: 0, column: 1 };
    let document = Document::new("mean()", None);
    let context = DocumentContext::new(&document, point, None);
    let completions = completions_from_comment(&context, &library).unwrap();
    assert!(completions.is_none());

    // If in a comment, return empty vector
    let point = Point { row: 0, column: 1 };
    let document = Document::new("# mean", None);
    let context = DocumentContext::new(&document, point, None);
    let completions = completions_from_comment(&context, &library)
        .unwrap()
        .unwrap();
    assert!(completions.is_empty());
}

#[test]
fn test_roxygen_comment() {
    use oak_package_metadata::description::Description;
    use oak_package_metadata::namespace::Namespace;
    use oak_semantic::package::Package;
    use tempfile::TempDir;
    use tree_sitter::Point;

    use crate::lsp::document::Document;

    // Straight from https://github.com/r-lib/roxygen2/blob/main/inst/roxygen2-tags.yml
    let content = r#"
- name: aliases
  description: >
    Add additional aliases to the topic.

    Use `NULL` to suppress the default alias automatically generated by roxygen2.
  template: ' ${1:alias}'
  vignette: index-crossref
  recommend: true

- name: description
  description: >
    A short description of the purpose of the function. Usually around
    a paragraph, but can be longer if needed.
  template: "\n${1:A short description...}\n"
  vignette: rd-functions
  recommend: true
"#;

    let path = TempDir::new().unwrap();
    std::fs::write(path.path().join("roxygen2-tags.yml"), content).unwrap();

    let package = Package::from_parts(
        path.path().to_path_buf(),
        Description {
            name: "roxygen2".to_string(),
            ..Description::default()
        },
        Namespace::default(),
    );

    let library = Library::new(vec![], None);
    let library = library.insert("roxygen2", package);

    let point = Point { row: 0, column: 4 };
    let document = Document::new("#' @", None);
    let context = DocumentContext::new(&document, point, None);
    let completions = completions_from_comment(&context, &library)
        .unwrap()
        .unwrap();

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
    let description = description.first().unwrap();
    assert_eq!(
        description.insert_text,
        Some(String::from(
            "description\n#' ${1:A short description...}\n#' "
        ))
    );
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
