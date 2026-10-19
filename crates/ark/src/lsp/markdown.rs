//
// markdown.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

#![allow(dead_code)]

use ego_tree::NodeRef;
use scraper::node::Text;
use scraper::ElementRef;
use scraper::Node;
use stdext::join;

pub fn md_codeblock(language: &str, code: &str) -> String {
    join!("``` ", language, "\n", code, "\n", "```", "\n")
}

pub fn md_bold(text: &str) -> String {
    join!("**", text, "**")
}

pub fn md_italic(text: &str) -> String {
    join!("_", text, "_")
}

pub fn md_h1(text: &str) -> String {
    join!("# ", text)
}

pub fn md_h2(text: &str) -> String {
    join!("## ", text)
}

pub fn md_h3(text: &str) -> String {
    join!("### ", text)
}

pub fn md_h4(text: &str) -> String {
    join!("#### ", text)
}

pub fn md_h5(text: &str) -> String {
    join!("###### ", text)
}

pub fn md_h6(text: &str) -> String {
    join!("###### ", text)
}

pub fn md_newline() -> String {
    "\n\n".to_string()
}

pub fn elt_text(node: ElementRef) -> String {
    node.text().collect::<String>()
}

pub fn elt_prev(node: ElementRef) -> Option<ElementRef> {
    for sibling in node.prev_siblings() {
        if let Some(elt) = ElementRef::wrap(sibling) {
            return Some(elt);
        }
    }

    None
}

pub fn elt_next(node: ElementRef) -> Option<ElementRef> {
    for sibling in node.next_siblings() {
        if let Some(elt) = ElementRef::wrap(sibling) {
            return Some(elt);
        }
    }

    None
}

pub struct MarkdownConverter<'a> {
    node: NodeRef<'a, Node>,
}

impl<'a> MarkdownConverter<'a> {
    pub fn new(node: NodeRef<'a, Node>) -> Self {
        MarkdownConverter { node }
    }

    pub fn convert(&self) -> String {
        let mut buffer = String::new();
        self.convert_into(&mut buffer);
        buffer
    }

    /// Append to existing markdown, so that block elements like lists can
    /// tell whether they start on a new line
    pub fn convert_into(&self, buffer: &mut String) {
        self.convert_node(self.node, buffer);
    }

    fn convert_node(&self, node: NodeRef<'a, Node>, buffer: &mut String) {
        if node.value().is_element() {
            let element = ElementRef::wrap(node).unwrap();
            self.convert_element(element, buffer);
        } else if node.value().is_text() {
            let text = node.value().as_text().unwrap();
            self.convert_text(text, buffer);
        }
    }

    fn convert_element(&self, element: ElementRef<'a>, buffer: &mut String) {
        let name = element.value().name();
        match name {
            "code" => {
                buffer.push('`');
                self.convert_children(element, buffer);
                buffer.push('`');
            },

            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let count = name.chars().nth(1).unwrap_or('0').to_digit(10).unwrap_or(0);
                buffer.push_str("#".repeat(count as usize).as_str());
                buffer.push(' ');
                self.convert_children(element, buffer);
            },

            "p" => {
                buffer.push('\n');
                self.convert_children(element, buffer);
                buffer.push('\n');
            },

            "tr" => self.convert_row(element, buffer, |cell, buffer| {
                self.convert_node(*cell, buffer);
            }),

            "ol" => self.convert_list(element, buffer, true),

            "ul" => self.convert_list(element, buffer, false),

            _ => {
                self.convert_children(element, buffer);
            },
        }
    }

    fn convert_children(&self, node: ElementRef<'a>, buffer: &mut String) {
        for child in node.children() {
            self.convert_node(child, buffer)
        }
    }

    fn convert_list(&self, element: ElementRef<'a>, buffer: &mut String, ordered: bool) {
        // A list marker is only recognized at the start of a line
        if !buffer.is_empty() && !buffer.ends_with('\n') {
            buffer.push('\n');
        }

        let items = element.children().filter_map(ElementRef::wrap);
        for (index, item) in items.enumerate() {
            let marker = if ordered {
                format!("{}. ", index + 1)
            } else {
                String::from("- ")
            };

            // R wraps item contents in `<p>`, which adds surrounding newlines.
            // Item text has to start on the marker's line, and continuation
            // lines have to be indented to the marker's width, or the text
            // falls out of the list item.
            let mut contents = String::new();
            if item.value().name() == "li" {
                self.convert_children(item, &mut contents);
            } else {
                self.convert_element(item, &mut contents);
            }

            let indent = " ".repeat(marker.len());
            buffer.push_str(&marker);
            for (line_index, line) in contents.trim().lines().enumerate() {
                if line_index > 0 {
                    buffer.push('\n');
                    if !line.is_empty() {
                        buffer.push_str(&indent);
                    }
                }
                buffer.push_str(line);
            }
            buffer.push('\n');
        }

        buffer.push('\n');
    }

    fn convert_text(&self, text: &Text, buffer: &mut String) {
        buffer.push_str(text.to_string().as_str())
    }

    fn convert_tr(&self, element: ElementRef<'a>, buffer: &mut String) {
        self.convert_row(element, buffer, |cell, buffer| {
            self.convert_node(*cell, buffer);
        })
    }

    fn convert_row(
        &self,
        element: ElementRef<'a>,
        buffer: &mut String,
        mut callback: impl FnMut(ElementRef<'a>, &mut String),
    ) {
        buffer.push_str("| ");
        for child in element.children() {
            if child.value().is_element() {
                let child = ElementRef::wrap(child).unwrap();
                let mut contents = String::new();
                callback(child, &mut contents);
                contents = contents.replace("\n", " ");
                buffer.push_str(contents.as_str().trim());
                buffer.push_str(" | ");
            }
        }
        buffer.pop();
    }
}

#[cfg(test)]
mod tests {
    use scraper::Html;

    use crate::lsp::markdown::MarkdownConverter;

    fn convert(html: &str) -> String {
        let html = Html::parse_fragment(html);
        MarkdownConverter::new(*html.root_element()).convert()
    }

    #[test]
    fn test_unordered_list_items_with_paragraphs() {
        // The shape `tools::Rd2HTML()` produces for `\itemize{}`
        let html = r#"<ul>
<li> <p><code>a()</code> is the first item, which
wraps onto a second line.
</p>
</li>
<li> <p><code>b()</code> is the second item.
</p>
</li></ul>"#;

        assert_eq!(
            convert(html),
            "- `a()` is the first item, which\n  wraps onto a second line.\n- `b()` is the second item.\n\n"
        );
    }

    #[test]
    fn test_ordered_list_items_are_numbered() {
        let html = r#"<ol>
<li> <p>First
step.</p>
</li>
<li> <p>Second step.</p>
</li></ol>"#;

        assert_eq!(convert(html), "1. First\n   step.\n2. Second step.\n\n");
    }

    #[test]
    fn test_list_starts_on_its_own_line() {
        let html = r#"Some text:<ul>
<li> <p>Item.</p>
</li></ul>"#;

        assert_eq!(convert(html), "Some text:\n- Item.\n\n");
    }

    #[test]
    fn test_nested_lists_are_indented() {
        let html = r#"<ul>
<li> <p>Outer.</p>
<ul>
<li> <p>Inner.</p>
</li></ul>
</li></ul>"#;

        assert_eq!(convert(html), "- Outer.\n\n  - Inner.\n\n");
    }

    #[test]
    fn test_list_directly_inside_list_is_kept() {
        // Malformed, but the inner list should still be rendered as a list
        let html = r#"<ul><ul><li>A</li><li>B</li></ul></ul>"#;

        assert_eq!(convert(html), "- - A\n  - B\n\n");
    }
}
