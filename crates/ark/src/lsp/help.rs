//
// help.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::utils::r_typeof;
use libr::NILSXP;
use regex::Regex;
use scraper::ElementRef;
use scraper::Html;
use scraper::Selector;
use stdext::push;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;

use crate::lsp::markdown::*;

pub struct RHtmlHelp {
    html: Html,

    /// Is this help page known to be for a function?
    function: bool,
}

pub enum Status {
    Done,
    KeepGoing,
}

impl RHtmlHelp {
    /// SAFETY: Requires access to the R runtime.
    pub fn from_topic(topic: &str, package: Option<&str>) -> anyhow::Result<Option<Self>> {
        Self::get_help(topic, package).map(|html| {
            html.map(|html| Self {
                html,
                function: false,
            })
        })
    }

    /// SAFETY: Requires access to the R runtime.
    pub fn from_function(name: &str, package: Option<&str>) -> anyhow::Result<Option<Self>> {
        Self::get_help(name, package).map(|html| {
            html.and_then(|html| {
                if Self::is_function(&html) {
                    Some(Self {
                        html,
                        function: true,
                    })
                } else {
                    None
                }
            })
        })
    }

    /// SAFETY: Requires access to the R runtime.
    fn get_help(topic: &str, package: Option<&str>) -> anyhow::Result<Option<Html>> {
        // trim off a package prefix if necessary
        let package = package.map(|s| s.replace("package:", ""));

        // get help document
        let contents = RFunction::from(".ps.help.getHtmlHelpContents")
            .param("topic", topic)
            .param("package", package)
            .call();

        let contents = unwrap!(contents, Err(err) => {
            log::error!("{err:?}");
            return Ok(None);
        });

        // check for NULL (implies no help available)
        if r_typeof(*contents) == NILSXP {
            return Ok(None);
        }

        // parse as html
        let contents = String::try_from(&contents)?;
        let html = Html::parse_document(contents.as_str());

        Ok(Some(html))
    }

    /// Is this a help page for a function?
    ///
    /// Uses a heuristic of looking for a `Usage` section to determine if this looks like
    /// function help or not. Can't look for `Arguments`, as some functions don't have
    /// any!
    fn is_function(x: &Html) -> bool {
        // Find all h3 headers in the document
        let selector = Selector::parse("h3").unwrap();
        let mut headers = x.select(&selector);

        // Do any have a usage section?
        headers.any(|header| header.html() == "<h3>Usage</h3>")
    }

    pub fn topic(&self) -> Option<String> {
        // get topic + title; normally available in first table in the document
        let selector = Selector::parse("table").unwrap();
        let preamble = self.html.select(&selector).next()?;

        // try to get the first cell
        let selector = Selector::parse("td").unwrap();
        let cell = preamble.select(&selector).next()?;
        let preamble = elt_text(cell);

        Some(preamble)
    }

    pub fn title(&self) -> Option<String> {
        let selector = Selector::parse("head > title").unwrap();
        let title = self.html.select(&selector).next()?;
        let mut title = elt_text(title);

        // R prepends 'R: ' to the title, so remove it if that exists
        if title.starts_with("R: ") {
            title.replace_range(0..3, "");
        }

        Some(title)
    }

    #[allow(unused)]
    pub fn section(&self, name: &str) -> Option<Vec<ElementRef>> {
        // find all h3 headers in the document
        let selector = Selector::parse("h3").unwrap();
        let mut headers = self.html.select(&selector);

        // search for the header with the matching name
        let needle = format!("<h2>{}</h2>", name);
        let header = headers.find(|elt| elt.inner_html() == needle);

        let header = match header {
            Some(header) => header,
            None => return None,
        };

        // start collecting elements
        let mut elements: Vec<ElementRef> = Vec::new();
        let mut elt = header;

        loop {
            elt = match elt_next(elt) {
                Some(elt) => elt,
                None => break,
            };

            if matches!(elt.value().name(), "h1" | "h2" | "h3") {
                break;
            }

            elements.push(elt);
        }

        Some(elements)
    }

    /// Find and parse the arguments in the HTML help
    ///
    /// The help file has the structure:
    ///
    /// <h3>Arguments</h3>
    ///
    /// <table>
    /// <tr style="vertical-align: top;"><td><code>parameter</code></td>
    /// <td>
    /// Parameter documentation.
    /// </td></tr>
    ///
    /// Note that parameters might be parsed as part of different, multiple tables;
    /// we need to iterate over all tables after the Arguments header.
    ///
    /// SAFETY: Errors if `self.function` is `false`.
    pub fn parameters(
        &self,
        mut callback: impl FnMut(&Vec<&str>, &ElementRef) -> Status,
    ) -> anyhow::Result<()> {
        if !self.function {
            return Err(anyhow::anyhow!(
                "Called `parameters()` on a topic that isn't a function."
            ));
        }

        let selector = Selector::parse("h3").unwrap();
        let mut headers = self.html.select(&selector);
        let header = headers
            .find(|node| node.html() == "<h3>Arguments</h3>")
            .into_result()?;

        let mut elt = header;
        loop {
            // Get the next element.
            elt = unwrap!(elt_next(elt), None => break);

            // If it's a header, time to stop parsing.
            if elt.value().name() == "h3" {
                break;
            }

            // If it's not a table, skip it.
            if elt.value().name() != "table" {
                continue;
            }

            // Get the cells in this table.
            // I really wish R included classes on these table elements...
            let selector = Selector::parse(r#"tr[style="vertical-align: top;"] > td"#).unwrap();
            let mut cells = elt.select(&selector);

            // Start iterating through pairs of cells.
            loop {
                // Get the parameters. Note that multiple parameters might be contained
                // within a single table cell, so we'll need to split that later.
                let lhs = unwrap!(cells.next(), None => { break });
                let names: String = lhs.text().collect();

                // Get the parameters associated with this description.
                let pattern = Regex::new("\\s*,\\s*").unwrap();
                let names = pattern.split(names.as_str()).collect::<Vec<_>>();

                // Get the parameter description.
                let rhs = unwrap!(cells.next(), None => { break });

                // Execute the callback.
                match callback(&names, &rhs) {
                    Status::Done => return Ok(()),
                    Status::KeepGoing => {},
                };
            }

            // If we got here, we managed to find and parse the argument table.
            break;
        }

        Ok(())
    }

    /// Extract content for an individual parameter by name
    ///
    /// SAFETY: Errors if `self.function` is `false`.
    pub fn parameter(&self, name: &str) -> anyhow::Result<Option<MarkupContent>> {
        if !self.function {
            return Err(anyhow::anyhow!(
                "Called `parameter()` on a topic that isn't a function."
            ));
        }

        let mut result = None;

        self.parameters(|params, node| {
            for param in params {
                if *param == name {
                    result = Some(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: MarkdownConverter::new(**node).convert(),
                    });
                    return Status::Done;
                }
            }

            return Status::KeepGoing;
        })?;

        Ok(result)
    }

    pub fn markdown(&self) -> anyhow::Result<String> {
        let mut markdown = String::new();

        // add topic
        if let Some(topic) = self.topic() {
            push!(markdown, md_italic(&topic), md_newline());
        }

        if let Some(title) = self.title() {
            push!(markdown, md_h2(&title), md_newline(), "------\n");
        }

        // iterate through the different sections in the help file
        for_each_section(&self.html, |header, elements| {
            // add a title
            let header = elt_text(header);
            markdown.push_str(md_h3(header.as_str()).as_str());
            markdown.push_str(md_newline().as_str());

            // add body
            let body = if matches!(header.as_str(), "Usage" | "Examples") {
                let mut buffer = String::new();
                for elt in elements {
                    if elt.value().name() == "hr" {
                        break;
                    }
                    let code = md_codeblock("r", elt_text(elt).as_str());
                    buffer.push_str(code.as_str());
                }
                buffer
            } else if matches!(header.as_str(), "Arguments") {
                // create a buffer for table output
                let mut buffer = String::new();

                // add an empty header
                buffer.push_str("|     |     |\n");
                buffer.push_str("| --- | --- |");

                // generate the markdown table
                for elt in elements {
                    let converter = MarkdownConverter::new(*elt);
                    let table = converter.convert();
                    buffer.push_str(table.as_str());
                }

                buffer
            } else {
                let mut buffer = String::new();
                for elt in elements {
                    let converter = MarkdownConverter::new(*elt);
                    let markdown = converter.convert();
                    buffer.push_str(markdown.as_str());
                }

                buffer
            };

            markdown.push_str(body.as_str());
            markdown.push_str(md_newline().as_str());
        });

        Ok(markdown)
    }
}

fn for_each_section(doc: &Html, mut callback: impl FnMut(ElementRef, Vec<ElementRef>)) {
    // find all h3 headers in the document
    let selector = Selector::parse("h3").unwrap();
    let headers = doc.select(&selector);

    // iterate through them, and pass each (+ the 'body' of the node) to the callback
    for header in headers {
        // collect all the elements following up to the next header
        let mut elements: Vec<ElementRef> = Vec::new();

        // start with the current header
        let mut elt = header;

        // find the next element -- we might need to skip interleaving nodes
        loop {
            // get the next element (if any)
            elt = unwrap!(elt_next(elt), None => { break });

            // if we find a header, assume that's the start of the next section
            if matches!(elt.value().name(), "h1" | "h2" | "h3") {
                break;
            }

            // add it to our list of elements
            elements.push(elt);
        }

        // execute the callback
        callback(header, elements);
    }
}

#[cfg(test)]
mod tests {
    use crate::lsp::help::RHtmlHelp;
    use crate::lsp::help::Status;
    use crate::r_task;

    #[test]
    fn test_help_from_function() {
        r_task(|| {
            let help = RHtmlHelp::from_function("match", None);
            let help = help.unwrap().unwrap();
            assert!(help.function);

            // Not found at all
            let help = RHtmlHelp::from_function("doesnt_exist", None);
            let help = help.unwrap();
            assert!(help.is_none());

            // Found, but not a function!
            let help = RHtmlHelp::from_function("plotmath", None);
            let help = help.unwrap();
            assert!(help.is_none());
            // It is a topic though
            let help = RHtmlHelp::from_topic("plotmath", None);
            let help = help.unwrap();
            assert!(help.is_some());
        });
    }

    #[test]
    fn test_markdown_conversion() {
        r_task(|| {
            let help = RHtmlHelp::from_function("match", None);
            let help = help.unwrap().unwrap();

            let markdown = help.markdown().unwrap();
            markdown.contains("### Usage");
        });
    }

    #[test]
    fn test_parameters_on_non_functions() {
        r_task(|| {
            let help = RHtmlHelp::from_topic("plotmath", None);
            let help = help.unwrap().unwrap();
            // Errors immediately
            insta::assert_snapshot!(help.parameters(|_, _| Status::Done).unwrap_err());
        });
    }

    #[test]
    fn test_parameter_on_non_functions() {
        r_task(|| {
            let help = RHtmlHelp::from_topic("plotmath", None);
            let help = help.unwrap().unwrap();
            // Errors immediately
            insta::assert_snapshot!(help.parameter("foo").unwrap_err());
        });
    }
}
