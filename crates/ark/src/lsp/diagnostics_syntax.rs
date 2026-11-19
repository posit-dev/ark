//
// diagnostics_syntax.rs
//
// Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
//
//

use std::io;

use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use anyhow::anyhow;
use biome_diagnostics::console::fmt::Display;
use biome_diagnostics::console::fmt::Formatter;
use biome_diagnostics::console::MarkupBuf;
use biome_diagnostics::display::markup_to_string;
use biome_diagnostics::LogCategory;
use biome_diagnostics::PrintDescription;
use biome_diagnostics::Severity;
use biome_diagnostics::Visit;
use biome_line_index::LineIndex;
use biome_parser::diagnostic::ParseDiagnostic;
use stdext::result::ResultExt;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::DiagnosticRelatedInformation;
use tower_lsp_server::ls_types::DiagnosticSeverity;
use tower_lsp_server::ls_types::Location;
use tower_lsp_server::ls_types::NumberOrString;
use tower_lsp_server::ls_types::Range;
use tower_lsp_server::ls_types::Uri;

/// Convert Air's parse diagnostics into LSP diagnostics
pub(crate) fn syntax_diagnostics(
    diagnostics: &[ParseDiagnostic],
    line_index: &LineIndex,
    encoding: PositionEncoding,
    uri: &Uri,
) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic_to_lsp(diagnostic, uri, line_index, encoding).log_err())
        .collect()
}

/// Convert a biome diagnostic into an LSP diagnostic
///
/// Roughly ported from `biome_lsp::utils::diagnostic_to_lsp()`
fn diagnostic_to_lsp(
    diagnostic: &impl biome_diagnostics::Diagnostic,
    uri: &Uri,
    line_index: &LineIndex,
    position_encoding: PositionEncoding,
) -> anyhow::Result<Diagnostic> {
    // Figure out where it is
    let Some(span) = diagnostic.location().span else {
        return Err(anyhow!("Diagnostic location has no span"));
    };
    let range = to_proto::range(span, line_index, position_encoding)?;

    // Figure out how severe it is
    let severity = Some(match diagnostic.severity() {
        Severity::Fatal | Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Information => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    });

    // Part of the label in the Problems pane, made up of `{source}({code})`,
    // i.e. that will show `ark(parse)` for these parse errors
    let source = Some(String::from("ark"));
    let code = diagnostic
        .category()
        .map(|category| NumberOrString::String(category.name().to_string()));

    // The base message of the diagnostic
    let message = PrintDescription(diagnostic).to_string();
    if message.is_empty() {
        return Err(anyhow!("Diagnostic description is empty"));
    }

    // Extract out message hints and details into LSP `DiagnosticRelatedInformation`
    let mut visitor = RelatedInformationVisitor::new(uri, range, line_index, position_encoding);
    diagnostic.advices(&mut visitor)?;
    let related_information = visitor.finish();

    Ok(Diagnostic {
        range,
        severity,
        code,
        source,
        message,
        related_information,
        ..Default::default()
    })
}

/// Turns a diagnostic's advice into `DiagnosticRelatedInformation`
///
/// Roughly based on biome's `RelatedInformationVisitor`
///
/// The general idea is that we expect "related" info for the diagnostic to be
/// collected as:
/// - A call to `record_log()` to record the message
/// - An optional call to `record_frame()` to record the location
///
/// We record the message and location as "pending" until we get either the next
/// `record_log()` call, or we run out of advice to iterate over. At either of
/// those points we finalize the `DiagnosticRelatedInformation` object and store
/// it.
struct RelatedInformationVisitor<'a> {
    uri: &'a Uri,
    primary_range: Range,

    line_index: &'a LineIndex,
    position_encoding: PositionEncoding,

    pending_message: Option<String>,
    pending_location: Option<Location>,

    // The eventual output
    related_information: Vec<DiagnosticRelatedInformation>,
}

impl<'a> RelatedInformationVisitor<'a> {
    fn new(
        uri: &'a Uri,
        primary_range: Range,
        line_index: &'a LineIndex,
        position_encoding: PositionEncoding,
    ) -> Self {
        Self {
            uri,
            primary_range,
            line_index,
            position_encoding,
            pending_message: None,
            pending_location: None,
            related_information: Vec::new(),
        }
    }

    fn finish(mut self) -> Option<Vec<DiagnosticRelatedInformation>> {
        // Push final `DiagnosticRelatedInformation`, if any
        self.push_pending();

        if self.related_information.is_empty() {
            None
        } else {
            Some(self.related_information)
        }
    }

    fn push_pending(&mut self) {
        let location = self.pending_location.take();
        let Some(message) = self.pending_message.take() else {
            // We always expect `record_log()` to be called before
            // `record_frame()`, so if we get a location without a message we
            // clear the location too
            return;
        };

        // When we don't have a location we just use the primary message's range
        let location = location.unwrap_or_else(|| Location {
            uri: self.uri.clone(),
            range: self.primary_range,
        });

        self.related_information
            .push(DiagnosticRelatedInformation { location, message });
    }
}

impl Visit for RelatedInformationVisitor<'_> {
    /// Store the pending message for the upcoming [DiagnosticRelatedInformation]
    ///
    /// We also treat this as our reset point between multiple [DiagnosticRelatedInformation]s,
    /// so we first clear out the last one before pushing the next one
    fn record_log(&mut self, _category: LogCategory, text: &dyn Display) -> io::Result<()> {
        // Push finished `DiagnosticRelatedInformation`
        self.push_pending();

        let mut markup = MarkupBuf::default();
        text.fmt(&mut Formatter::new(&mut markup))?;
        let message = markup_to_string(&markup)
            .ok_or_else(|| io::Error::other("Can't render markup as text"))?;

        self.pending_message = Some(message);

        Ok(())
    }

    /// Store the pending location for the upcoming [DiagnosticRelatedInformation]
    fn record_frame(&mut self, location: biome_diagnostics::Location<'_>) -> io::Result<()> {
        // If we can't figure out a location, it just uses the primary message's range
        let Some(span) = location.span else {
            return Ok(());
        };
        let Ok(range) = to_proto::range(span, self.line_index, self.position_encoding) else {
            return Ok(());
        };
        self.pending_location = Some(Location {
            uri: self.uri.clone(),
            range,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use aether_lsp_utils::proto::PositionEncoding;
    use aether_parser::RParserOptions;
    use biome_line_index::LineIndex;
    use biome_line_index::WideEncoding;
    use tower_lsp_server::ls_types::Diagnostic;
    use tower_lsp_server::ls_types::Range;
    use tower_lsp_server::ls_types::Uri;

    use crate::lsp::diagnostics_syntax::syntax_diagnostics;

    const ENCODING: PositionEncoding = PositionEncoding::Wide(WideEncoding::Utf16);

    fn text_diagnostics(text: &str) -> Vec<Diagnostic> {
        let parse = aether_parser::parse(text, RParserOptions::default());
        let line_index = LineIndex::new(text);
        let uri = "file:///test.R".parse::<Uri>().unwrap();
        syntax_diagnostics(parse.diagnostics(), &line_index, ENCODING, &uri)
    }

    fn render_range(range: Range) -> String {
        format!(
            "{}:{}-{}:{}",
            range.start.line, range.start.character, range.end.line, range.end.character
        )
    }

    /// Render `text` in a snapshottable form, with a marker line under each
    /// line a diagnostic points at
    fn render(text: &str) -> String {
        let mut markers: Vec<Marker> = Vec::new();

        for (index, diagnostic) in text_diagnostics(text).into_iter().enumerate() {
            let id = index + 1;
            markers.push(Marker {
                id: format!("[{id}]"),
                range: diagnostic.range,
                message: diagnostic.message,
            });

            for (related_index, related) in diagnostic
                .related_information
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                let related_id = related_index + 1;
                markers.push(Marker {
                    id: format!("[{id}.{related_id}]"),
                    range: related.location.range,
                    message: related.message,
                });
            }
        }

        // Stable, so markers at the same column keep diagnostic order
        markers.sort_by_key(|marker| (marker.range.start.line, marker.range.start.character));

        let lines: Vec<&str> = text.split('\n').collect();
        let mut out = String::from("```text\n");

        for (line_number, line) in lines.iter().enumerate() {
            let line_markers: Vec<&Marker> = markers
                .iter()
                .filter(|marker| marker.range.start.line as usize == line_number)
                .collect();

            // Skip the empty line after a trailing newline unless something points at it
            let is_last = line_number == lines.len() - 1;
            if is_last && line.is_empty() && line_markers.is_empty() {
                continue;
            }

            out.push_str(line);
            out.push('\n');

            for marker in line_markers {
                out.push_str(&marker.render(line));
            }
        }

        out.push_str("```\n");
        out
    }

    struct Marker {
        id: String,
        range: Range,
        message: String,
    }

    impl Marker {
        fn render(&self, line: &str) -> String {
            let start = char_column(line, self.range.start.character);
            let end = if self.range.end.line == self.range.start.line {
                char_column(line, self.range.end.character)
            } else {
                line.chars().count()
            };
            let width = end.saturating_sub(start).max(1);

            format!(
                "{}{} {} {} ({})\n",
                " ".repeat(start),
                "^".repeat(width),
                self.id,
                self.message,
                render_range(self.range)
            )
        }
    }

    /// Convert a UTF-16 column to a character column, for padding
    fn char_column(line: &str, utf16_column: u32) -> usize {
        let mut units = 0;
        for (index, char) in line.chars().enumerate() {
            if units >= utf16_column as usize {
                return index;
            }
            units += char.len_utf16();
        }
        line.chars().count() + (utf16_column as usize).saturating_sub(units)
    }

    #[test]
    fn test_detail_becomes_related_information() {
        insta::assert_snapshot!(render("\"abc"));
    }

    #[test]
    fn test_hint_points_at_primary_range() {
        insta::assert_snapshot!(render("1 %foo"));
    }

    #[test]
    fn test_multiple_errors() {
        insta::assert_snapshot!(render(
            "\
f(1 2)
1 + }"
        ));
    }

    #[test]
    fn test_utf16_columns() {
        insta::assert_snapshot!(render("\"é😀\" + }"));
    }

    #[test]
    fn test_long_unterminated_string() {
        let text = format!("('{})", "\n".repeat(20));
        insta::assert_snapshot!(render(&text));
    }

    #[test]
    fn test_unmatched_call_delimiter() {
        insta::assert_snapshot!(render("match(a, b"));
        insta::assert_snapshot!(render("foo[a, b"));
        insta::assert_snapshot!(render("foo[[a, b"));
    }

    #[test]
    fn test_unmatched_call_delimiter_with_trailing_info() {
        insta::assert_snapshot!(render(
            "\
match(1, 2

identity(1)
"
        ));
    }

    #[test]
    fn test_unmatched_braces() {
        insta::assert_snapshot!(render("{"));
        insta::assert_snapshot!(render("{ 1 + 2"));
    }

    #[test]
    fn test_unmatched_parentheses() {
        insta::assert_snapshot!(render("("));
        insta::assert_snapshot!(render("( 1 + 2"));
        insta::assert_snapshot!(render("()"));
        insta::assert_snapshot!(render(
            "\
(1+2
1+2)"
        ));
    }

    #[test]
    fn test_error_precision() {
        insta::assert_snapshot!(render("sum(1 * 2 + )"));
    }

    #[test]
    fn test_unmatched_closing_token() {
        insta::assert_snapshot!(render("1 + 1 }"));
        insta::assert_snapshot!(render("1 + 1 )"));
        insta::assert_snapshot!(render("1 + 1 ]"));
    }

    #[test]
    fn test_unmatched_closing_token_precision() {
        insta::assert_snapshot!(render("1 + }"));
    }

    #[test]
    fn test_unmatched_binary_operator() {
        insta::assert_snapshot!(render(
            "\
{
 1 +
}"
        ));
    }

    #[test]
    fn test_unmatched_function_parameters_parentheses() {
        insta::assert_snapshot!(render(
            "\
function(x {
  1 + 1
}"
        ));
    }

    #[test]
    fn test_for_loop_with_no_body() {
        insta::assert_snapshot!(render("for(i in 1:10)"));
    }

    #[test]
    fn test_while_loop_with_no_body() {
        insta::assert_snapshot!(render("while(1)"));
    }

    #[test]
    fn test_repeat_loop_with_no_body() {
        insta::assert_snapshot!(render(
            "\
1
repeat"
        ));
    }

    #[test]
    fn test_if_statement_with_no_consequence() {
        insta::assert_snapshot!(render("if (a)"));
    }

    #[test]
    fn test_function_definition_with_no_body() {
        insta::assert_snapshot!(render("function(a)"));
        insta::assert_snapshot!(render(r"\(a)"));
    }

    #[test]
    fn test_repeated_call_arguments_without_delimiter() {
        insta::assert_snapshot!(render("match(1, 2 3)"));
    }

    #[test]
    fn test_missing_namespace_rhs() {
        insta::assert_snapshot!(render("base::"));
    }

    #[test]
    fn test_missing_commas_reported_once_per_list() {
        // A missing `)` swallows the following lines as arguments
        insta::assert_snapshot!(render(
            "\
match(1, 2

identity(1)
identity(2)
identity(3)
"
        ));
        insta::assert_snapshot!(render("f(a b c d)"));
        insta::assert_snapshot!(render("function(a b c) 1"));
        // The inner call cascades while the outer call still reports its own `)`
        insta::assert_snapshot!(render(
            "\
f(g(1
2
3
"
        ));
    }
}
