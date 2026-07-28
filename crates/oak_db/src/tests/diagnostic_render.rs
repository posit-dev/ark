//! Renders `Diagnostic`s alongside the source they annotate, for use in
//! `insta` snapshot tests. Built on `annotate-snippets`, the same rustc-style
//! renderer ty and ruff use.

use std::ops::Range;

use annotate_snippets::AnnotationKind;
use annotate_snippets::Group;
use annotate_snippets::Level;
use annotate_snippets::Renderer;
use annotate_snippets::Snippet;
use biome_rowan::TextRange;

use crate::Diagnostic;
use crate::Severity;

/// Render each of `diagnostics` as its own rustc-style report against
/// `source`, joined by blank lines. `name` is the file name shown in each
/// report's `-->` locus (typically `"a.R"`).
pub(super) fn render(name: &str, source: &str, diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return format!("{name}\n(no diagnostics)");
    }

    let renderer = Renderer::plain();
    diagnostics
        .iter()
        .map(|diagnostic| renderer.render(&[group_for(name, source, diagnostic)]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Build one report `Group`: a title carrying the diagnostic's kind and
/// message, and a snippet with the call site as the primary annotation and
/// each `Annotation` as a labeled secondary one.
fn group_for<'a>(name: &'a str, source: &'a str, diagnostic: &'a Diagnostic) -> Group<'a> {
    let title = level_for(diagnostic.kind().severity())
        .primary_title(diagnostic.message())
        .id(diagnostic.kind().as_str());

    let mut snippet = Snippet::source(source)
        .path(name)
        .annotation(AnnotationKind::Primary.span(byte_range(diagnostic.range())));

    for annotation in diagnostic.annotations() {
        snippet = snippet.annotation(
            AnnotationKind::Context
                .span(byte_range(annotation.range))
                .label(annotation.message.as_str()),
        );
    }

    title.element(snippet)
}

fn level_for(severity: Severity) -> Level<'static> {
    match severity {
        Severity::Error => Level::ERROR,
        Severity::Warning => Level::WARNING,
        Severity::Info => Level::INFO,
        Severity::Hint => Level::HELP,
    }
}

fn byte_range(range: TextRange) -> Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}
