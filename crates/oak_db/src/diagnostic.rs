use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_semantic::semantic_index::AmbiguityReason;
use oak_semantic::semantic_index::SemanticDiagnostic;

use crate::load_context::loader;
use crate::load_context::LoaderInfo;
use crate::Db;
use crate::File;

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
    AmbiguousCalleeResolution,
    AmbiguousAttachOrder,
    AmbiguousSuperAssignment,
    LockedSuperAssignment,
    UninstalledPackage,
    SourceCycle,
    InheritedShadow,
}

impl DiagnosticKind {
    /// The stable string an LSP consumer reports as the diagnostic `code`.
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagnosticKind::AmbiguousCalleeResolution => "ambiguous-callee-resolution",
            DiagnosticKind::AmbiguousAttachOrder => "ambiguous-attach-order",
            DiagnosticKind::AmbiguousSuperAssignment => "ambiguous-super-assignment",
            DiagnosticKind::LockedSuperAssignment => "locked-super-assignment",
            DiagnosticKind::UninstalledPackage => "uninstalled-package",
            DiagnosticKind::SourceCycle => "source-cycle",
            DiagnosticKind::InheritedShadow => "inherited-shadow",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            DiagnosticKind::AmbiguousCalleeResolution => Severity::Info,
            DiagnosticKind::AmbiguousAttachOrder => Severity::Info,
            DiagnosticKind::AmbiguousSuperAssignment => Severity::Info,
            DiagnosticKind::LockedSuperAssignment => Severity::Warning,
            DiagnosticKind::UninstalledPackage => Severity::Warning,
            DiagnosticKind::SourceCycle => Severity::Warning,
            DiagnosticKind::InheritedShadow => Severity::Info,
        }
    }

    pub fn is_experimental(&self) -> bool {
        match self {
            DiagnosticKind::AmbiguousCalleeResolution => true,
            DiagnosticKind::AmbiguousAttachOrder => true,
            DiagnosticKind::AmbiguousSuperAssignment => true,
            DiagnosticKind::LockedSuperAssignment => true,
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
pub(crate) fn lower_semantic_diagnostic(
    db: &dyn Db,
    file: File,
    diagnostic: &SemanticDiagnostic,
) -> Diagnostic {
    match diagnostic {
        SemanticDiagnostic::AmbiguousCalleeResolution {
            name,
            call_range,
            reason,
        } => lower_ambiguous_callee_resolution(name, *call_range, reason),
        SemanticDiagnostic::AmbiguousAttachOrder { packages, range } => {
            lower_ambiguous_attach_order(packages, *range)
        },
        SemanticDiagnostic::AmbiguousSuperAssignment {
            name,
            range,
            binding_range,
        } => lower_ambiguous_super_assignment(name, *range, *binding_range),
        SemanticDiagnostic::LockedSuperAssignment {
            name,
            package,
            range,
        } => lower_locked_super_assignment(name, package, *range),
        SemanticDiagnostic::UninstalledPackage { package, range } => {
            lower_uninstalled_package(package, *range)
        },
        SemanticDiagnostic::SourceCycle => lower_source_cycle(loader(db, file)),
    }
}

/// The primary range is always the call site. The reason's competing site
/// becomes a single annotation.
fn lower_ambiguous_callee_resolution(
    name: &str,
    call_range: TextRange,
    reason: &AmbiguityReason,
) -> Diagnostic {
    let (message, annotation) = match reason {
        AmbiguityReason::LazyShadow { overwrite_range } => (
            format!(
                "Ambiguous reading of `{name}()`.\nAn assignment to `{name}` in an enclosing \
                 scope could run before this call and change what it does."
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

    Diagnostic::new(
        DiagnosticKind::AmbiguousCalleeResolution,
        message,
        call_range,
        vec![annotation],
    )
}

/// Annotate the selected target binding because it may not exist when the
/// assignment runs. The primary range points at the assigned name.
fn lower_ambiguous_super_assignment(
    name: &str,
    range: TextRange,
    binding_range: TextRange,
) -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::AmbiguousSuperAssignment,
        format!(
            "Ambiguous target of `{name} <<-`.\nAn assignment to `{name}` in an enclosing scope \
             could be absent when this closure runs, changing what `{name} <<-` updates."
        ),
        range,
        vec![Annotation {
            range: binding_range,
            message: format!("This binding may not exist when `{name} <<-` runs"),
        }],
    )
}

fn lower_locked_super_assignment(name: &str, package: &str, range: TextRange) -> Diagnostic {
    Diagnostic::new(
        DiagnosticKind::LockedSuperAssignment,
        format!("Cannot change value of locked binding for `{name}` in package `{package}`."),
        range,
        Vec::new(),
    )
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

/// Anchor at the file start because cycle records carry no source range.
///
/// Report every participant. Recovery rebuilds without cross-file resolution,
/// so we can't identify the exact source call or effect that formed the cycle.
fn lower_source_cycle(loader: Option<LoaderInfo>) -> Diagnostic {
    let cause = match loader {
        Some(LoaderInfo { name, loads }) => {
            format!(
                "{name} already loads {loads}.\n\
                 A `source()` call into a file that the loader also loads creates this cycle."
            )
        },
        None => "These files may `source()` each other, or `source()` a file that a loader such \
                 as a Shiny app or testthat suite already loads for them."
            .to_string(),
    };

    Diagnostic::new(
        DiagnosticKind::SourceCycle,
        format!(
            "This file is part of a cycle in how the project's files load each other.\n\
             {cause}\n\
             Language analysis will be incomplete until the cycle is resolved."
        ),
        TextRange::empty(TextSize::from(0)),
        Vec::new(),
    )
}
