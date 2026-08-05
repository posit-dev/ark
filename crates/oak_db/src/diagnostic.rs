use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::AmbiguityReason;
use oak_semantic::semantic_index::SemanticDiagnostic;

/// A diagnostic derived from a file's semantic analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    kind: DiagnosticKind,
    message: String,
    range: TextRange,
    annotations: Vec<Annotation>,
}

impl Diagnostic {
    pub(crate) fn new(
        kind: DiagnosticKind,
        message: String,
        range: TextRange,
        annotations: Vec<Annotation>,
    ) -> Self {
        Self {
            kind,
            message,
            range,
            annotations,
        }
    }

    pub fn kind(&self) -> DiagnosticKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }
}

/// A secondary site that gives context for a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub range: TextRange,
    pub message: String,
}

/// Identifies a diagnostic's kind. Drives its LSP `code`, severity, and
/// experimental status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    AmbiguousEffect,
    AmbiguousAttachOrder,
    UninstalledPackage,
    SourceCycle,
    InheritedShadow,
}

impl DiagnosticKind {
    /// The stable string an LSP consumer reports as the diagnostic `code`.
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagnosticKind::AmbiguousEffect => "ambiguous-effect",
            DiagnosticKind::AmbiguousAttachOrder => "ambiguous-attach-order",
            DiagnosticKind::UninstalledPackage => "uninstalled-package",
            DiagnosticKind::SourceCycle => "source-cycle",
            DiagnosticKind::InheritedShadow => "inherited-shadow",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            DiagnosticKind::AmbiguousEffect => Severity::Info,
            DiagnosticKind::AmbiguousAttachOrder => Severity::Info,
            DiagnosticKind::UninstalledPackage => Severity::Warning,
            DiagnosticKind::SourceCycle => Severity::Warning,
            DiagnosticKind::InheritedShadow => Severity::Info,
        }
    }

    pub fn is_experimental(&self) -> bool {
        match self {
            DiagnosticKind::AmbiguousEffect => true,
            DiagnosticKind::AmbiguousAttachOrder => true,
            DiagnosticKind::UninstalledPackage => true,
            DiagnosticKind::SourceCycle => true,
            DiagnosticKind::InheritedShadow => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

/// Lower one of `oak_semantic`'s raw diagnostic records into a `Diagnostic`.
pub(crate) fn lower_semantic_diagnostic(diagnostic: &SemanticDiagnostic) -> Diagnostic {
    match diagnostic {
        SemanticDiagnostic::AmbiguousEffect {
            name,
            call_range,
            reason,
        } => lower_ambiguous_effect(name, *call_range, reason),
        SemanticDiagnostic::AmbiguousAttachOrder { packages, range } => {
            lower_ambiguous_attach_order(packages, *range)
        },
        SemanticDiagnostic::UninstalledPackage { package, range } => {
            lower_uninstalled_package(package, *range)
        },
        SemanticDiagnostic::SourceCycle => lower_source_cycle(),
    }
}

/// The primary range is always the call site. The reason's competing site
/// becomes a single annotation.
fn lower_ambiguous_effect(
    name: &str,
    call_range: TextRange,
    reason: &AmbiguityReason,
) -> Diagnostic {
    let (message, annotation) = match reason {
        AmbiguityReason::LazyShadow { overwrite_range } => (
            format!(
                "Ambiguous reading of effectful `{name}()`.\nAn assignment to `{name}` in an enclosing \
                 scope could run before this call and change its effect."
            ),
            Annotation {
                range: *overwrite_range,
                message: "could run before the call".to_string(),
            },
        ),
        AmbiguityReason::ConditionalShadow { binding_range } => (
            format!(
                "Ambiguous reading of effectful `{name}()`.\nA conditional assignment could shadow `{name}` \
                 on some paths and change its effect."
            ),
            Annotation {
                range: *binding_range,
                message: format!("conditional assignment to `{name}`"),
            },
        ),
        AmbiguityReason::ConditionalAttach {
            package,
            attach_range,
        } => (
            format!(
                "Ambiguous reading of `{name}()`.\nThe package `{package}` is conditionally attached and does not import `{name}` \
                 across all paths."
            ),
            Annotation {
                range: *attach_range,
                message: format!("`{package}` attached only here"),
            },
        ),
    };

    Diagnostic::new(DiagnosticKind::AmbiguousEffect, message, call_range, vec![
        annotation,
    ])
}

/// The primary range covers the `if`. Both arms are inside it, so there is no
/// competing site to annotate.
fn lower_ambiguous_attach_order(packages: &[String], range: TextRange) -> Diagnostic {
    let message = format!(
        "Ambiguous attach order.\nThe branches attach {packages} in different orders, so which \
         package masks the other depends on the branch taken.",
        packages = packages
            .iter()
            .map(|package| format!("`{package}`"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    Diagnostic::new(
        DiagnosticKind::AmbiguousAttachOrder,
        message,
        range,
        Vec::new(),
    )
}

fn lower_uninstalled_package(package: &str, range: TextRange) -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::UninstalledPackage,
        format!("Package `{package}` is not installed.\nLanguage analysis will be incomplete."),
        range,
        Vec::new(),
    )
}

/// Anchored at the start of the file because the record carries no range.
/// Every file in the cycle gets its own diagnostic.
fn lower_source_cycle() -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::SourceCycle,
        "This file takes part in a cycle of mutual `source()` calls. \
         Language analysis will be incomplete until the cycle is resolved."
            .to_string(),
        TextRange::empty(TextSize::from(0)),
        Vec::new(),
    )
}
