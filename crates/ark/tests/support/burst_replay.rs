//! Replays a burst of document churn through the production event handler.
//!
//! Temporary documents open under unique URIs and close a few events later.
//! Padding lines make each one costlier to analyse than its code alone. The
//! replay also edits one recurring document, probes it with goto-definition
//! after each edit, and fetches package sources.
//!
//! The whole burst is queued before the loop handles any of it, so
//! notifications accumulate behind the main loop. [`Replay::drive_to_endpoint()`]
//! then runs the loop until the final recurring-document diagnostics are
//! accepted and every probe has been answered. The workspace scan and source
//! fetches the burst triggers run concurrently and may still be in flight at
//! that point. [`Replay::settle()`] drains them.
//!
//! Assertions accept any background completion order but require the final
//! diagnostics, navigation answers, and publications.
//!
//! Shared by the `burst` integration test and the `burst` benchmark case.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;

use ark::lsp::harness::DefinitionRequest;
use ark::lsp::harness::LspHarness;
use ark::lsp::harness::LspSession;
use ark::lsp::harness::Publication;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tempfile::TempDir;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::DiagnosticSeverity;
use tower_lsp_server::ls_types::GotoDefinitionResponse;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::Range;
use tower_lsp_server::ls_types::Uri;

/// Packages referenced through `::` to trigger source-pool fetches.
const DONORS: [&str; 2] = ["donor1", "donor2"];

/// The single source file each donor serves.
const DONOR_SOURCE: &str = "foo <- function() 1\n";

/// Workspace document kept open so refreshes cover a file outside the burst.
const SCRIPT: &str = "burst_script <- function() 1\nburst_script()\n";

const HELPER: &str = "burst_helper";

/// Code-relative lines used for the helper definition, probe call, and missing
/// symbol assertions.
const DEFINITION_LINE: u32 = 0;
const CALL_LINE: u32 = 1;
const MISSING_LINE: u32 = 2;

/// Maximum open temporary documents. Multiple live URIs create backlog that
/// keyed replacement cannot coalesce.
const WINDOW: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct BurstConfig {
    /// Temporary documents opened and closed during the burst. The recurring
    /// document gets one edit and one probe per temporary document.
    pub temporaries: usize,

    /// Padding lines per code line. Scales document size, and with it the cost
    /// of each analysis pass, without changing the code or its diagnostics.
    pub padding_ratio: usize,
}

impl BurstConfig {
    pub const DEFAULT: Self = Self {
        temporaries: 8,
        padding_ratio: 40,
    };

    /// Use an edit-specific missing symbol to reject stale diagnostics
    /// publications.
    pub fn recurring(&self, edit: usize) -> PaddedDocument {
        let missing = missing_symbol(edit);
        let code = format!("{HELPER} <- function() 1\n{HELPER}()\n{missing}\n");
        pad(&code, self.padding_ratio)
    }

    fn temporary(&self, index: usize) -> PaddedDocument {
        let code = format!("burst_temporary_{index} <- 1\nburst_absent_{index}\n");
        pad(&code, self.padding_ratio)
    }

    /// Reserve the version after the recurring edits interleaved with
    /// temporary documents.
    pub fn final_edit(&self) -> usize {
        self.temporaries + 1
    }
}

fn missing_symbol(edit: usize) -> String {
    format!("burst_missing_{edit}")
}

type KeyFields = (String, Option<DiagnosticSeverity>, Range);

/// Document text with padding around the code, and the line where the code
/// starts.
pub struct PaddedDocument {
    pub text: String,

    code_line: u32,
}

fn pad(code: &str, ratio: usize) -> PaddedDocument {
    let padding = code.lines().count() * ratio;
    let above = padding / 2;

    let mut text: String = (0..above).map(padding_line).collect();
    text.push_str(code);
    text.extend((above..padding).map(padding_line));

    PaddedDocument {
        text,
        code_line: above as u32,
    }
}

/// Alternate comment and blank lines so padding covers both kinds of trivia.
fn padding_line(index: usize) -> &'static str {
    if index.is_multiple_of(2) {
        "#\n"
    } else {
        "\n"
    }
}

pub struct Fixture {
    workspace: TempDir,
    library: TempDir,
    /// Burst-document URIs use this directory, but their text arrives through
    /// `didOpen` notifications.
    documents: TempDir,
    /// Donor sources, written before the session starts so the source handler
    /// only resolves directories.
    sources: TempDir,
}

impl Fixture {
    pub fn new() -> Self {
        let library = tempfile::tempdir().unwrap();
        for donor in DONORS {
            write_description(&library.path().join(donor), &[
                ("Package", donor),
                ("Version", "0.0.0"),
                ("Built", "dummy"),
            ]);
        }

        // References through `::` make the workspace scan request donor sources.
        let workspace = tempfile::tempdir().unwrap();
        let package = workspace.path().join("myproj");
        write_description(&package, &[("Package", "myproj"), ("Version", "0.0.0")]);
        let uses: String = DONORS
            .iter()
            .map(|donor| format!("{donor}::foo()\n"))
            .collect();
        write_file(&package.join("R").join("use.R"), &uses);

        let sources = tempfile::tempdir().unwrap();
        for donor in DONORS {
            write_file(&sources.path().join(donor).join("foo.R"), DONOR_SOURCE);
        }

        Self {
            workspace,
            library,
            documents: tempfile::tempdir().unwrap(),
            sources,
        }
    }

    pub fn script(&self) -> PathBuf {
        self.workspace.path().join("script.R")
    }

    /// Keep one URI stable so keyed replacement can coalesce its updates.
    pub fn recurring(&self) -> PathBuf {
        self.documents.path().join("recurring.R")
    }

    /// Use a unique URI to prevent keyed replacement from coalescing temporary
    /// documents.
    fn temporary(&self, index: usize) -> PathBuf {
        self.documents.path().join(format!("temporary_{index}.R"))
    }

    fn source_directories(&self) -> HashMap<String, PathBuf> {
        DONORS
            .iter()
            .map(|donor| (donor.to_string(), self.sources.path().join(donor)))
            .collect()
    }
}

fn write_description(dir: &Path, fields: &[(&str, &str)]) {
    let contents: String = fields
        .iter()
        .map(|(key, value)| format!("{key}: {value}\n"))
        .collect();
    write_file(&dir.join("DESCRIPTION"), &contents);
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

pub struct Replay {
    pub session: LspSession,
    pub config: BurstConfig,

    pub probes: Vec<Probe>,

    /// Retained separately from later publications so settlement cannot hide
    /// incorrect diagnostics accepted at the measured endpoint.
    endpoint_diagnostics: Option<Vec<Diagnostic>>,

    final_version: i32,
}

pub struct Probe {
    pub request: DefinitionRequest,

    definition: Range,
}

impl Replay {
    pub async fn start(fixture: &Fixture, config: BurstConfig) -> Self {
        let mut db = OakDatabase::new();
        db.set_library_paths(&[fixture.library.path().to_path_buf()]);

        let session = LspHarness::with_default_source_fetching(db)
            .start_with_package_sources(&[], fixture.source_directories())
            .await;

        Self {
            session,
            config,
            probes: Vec::new(),
            endpoint_diagnostics: None,
            final_version: config.final_edit() as i32,
        }
    }

    /// Queue the full burst without handling any of it.
    pub fn enqueue_burst(&mut self, fixture: &Fixture) {
        let config = self.config;
        let recurring = fixture.recurring();

        self.session
            .send_did_change_workspace_folders(fixture.workspace.path());
        self.session.send_did_open(&fixture.script(), SCRIPT);
        self.session
            .send_did_open(&recurring, &config.recurring(0).text);

        let mut open: VecDeque<PathBuf> = VecDeque::new();
        for index in 0..config.temporaries {
            let temporary = fixture.temporary(index);
            self.session
                .send_did_open(&temporary, &config.temporary(index).text);
            open.push_back(temporary);

            let edit = config.recurring(index + 1);
            self.session
                .send_did_change(&recurring, &edit.text, (index + 1) as i32);
            self.enqueue_probe(&recurring, &edit);

            if open.len() > WINDOW {
                self.close_oldest(&mut open);
            }
        }
        while !open.is_empty() {
            self.close_oldest(&mut open);
        }

        let last = config.recurring(config.final_edit());
        self.session
            .send_did_change(&recurring, &last.text, self.final_version);
    }

    /// Probe the recurring document's in-buffer helper, independent of
    /// workspace-scan progress.
    fn enqueue_probe(&mut self, path: &Path, document: &PaddedDocument) {
        let call = Position::new(document.code_line + CALL_LINE, 0);
        let request = self.session.send_goto_definition(path, call);

        self.probes.push(Probe {
            request,
            definition: range(
                (document.code_line + DEFINITION_LINE, 0),
                (document.code_line + DEFINITION_LINE, HELPER.len() as u32),
            ),
        });
    }

    fn close_oldest(&self, open: &mut VecDeque<PathBuf>) {
        if let Some(oldest) = open.pop_front() {
            self.session.send_did_close(&oldest);
        }
    }

    /// Run the loop until the final recurring edit's diagnostics are accepted
    /// and every probe is answered.
    ///
    /// Workspace scans, donor source ingestion, and their diagnostics refreshes
    /// may continue after this endpoint. Use [`Self::settle()`] to finish them.
    ///
    /// Probes and `didClose` notifications precede the final edit in the queue
    /// and are handled synchronously. Their replies and clearing publications
    /// must therefore exist before the final edit's diagnostics are accepted.
    pub async fn drive_to_endpoint(&mut self, fixture: &Fixture) {
        let diagnostics = self
            .session
            .wait_for_accepted_diagnostics(&fixture.recurring(), self.final_version)
            .await;
        self.endpoint_diagnostics = Some(diagnostics);

        for probe in &self.probes {
            if self.session.definition_answer(probe.request).is_none() {
                panic!(
                    "{request:?} was unanswered at the endpoint",
                    request = probe.request
                );
            }
        }
    }

    /// Drain the scans, source fetches, and refreshes left after the endpoint.
    pub async fn settle(&mut self) {
        self.session.settle().await;
    }

    pub fn assert_final_state(&self, fixture: &Fixture) {
        let recurring = uri(&fixture.recurring());

        assert_eq!(
            self.endpoint_diagnostics.as_deref().map(key_fields),
            Some(final_diagnostics(&self.config))
        );

        // Refreshes caused by source ingestion must preserve the final edit's
        // diagnostics and version.
        let last = self.last_publication(&recurring);
        assert_eq!(
            key_fields(last.diagnostics()),
            final_diagnostics(&self.config)
        );
        assert_eq!(last.version(), Some(self.final_version));

        // The diagnostics and probes above resolve only local symbols, so they
        // pass without the workspace scan or source ingestion. Check that the
        // burst's background workload actually ran, or a regression that stops
        // fetching donors would pass here and make the benchmark look faster.
        for donor in DONORS {
            assert_eq!(
                self.session.package_sources(donor),
                Some(vec![DONOR_SOURCE])
            );
        }

        // Check for a clearing publication, not necessarily an empty final
        // publication. An in-flight pass can publish after `didClose`.
        for index in 0..self.config.temporaries {
            let temporary = uri(&fixture.temporary(index));
            assert!(self.session.publications().any(|publication| {
                *publication.uri() == temporary && publication.diagnostics().is_empty()
            }));
        }

        let mut open: Vec<&str> = self
            .session
            .open_documents()
            .into_iter()
            .map(|uri| uri.as_str())
            .collect();
        open.sort();
        let mut expected = vec![
            recurring.as_str().to_string(),
            uri(&fixture.script()).as_str().to_string(),
        ];
        expected.sort();
        assert_eq!(open, expected);

        assert_eq!(self.probes.len(), self.config.temporaries);
        for probe in &self.probes {
            self.assert_probe_found_definition(probe);
        }

        let diagnostics = self.session.diagnostics_metrics();
        assert!(diagnostics.results_accepted > 0);

        let queue = self.session.analysis_metrics();
        assert_eq!(queue.waiting(), 0);
        assert_eq!(queue.running(), 0);
        assert!(queue.completed > 0);
        assert!(queue.queued >= diagnostics.tasks_queued);
    }

    #[track_caller]
    fn assert_probe_found_definition(&self, probe: &Probe) {
        let Some(answer) = self.session.definition_answer(probe.request) else {
            panic!("{request:?} was never answered", request = probe.request);
        };
        let Ok(Some(GotoDefinitionResponse::Link(links))) = &answer.response else {
            panic!(
                "The goto-definition probe found no definition: {response:?}",
                response = answer.response
            );
        };
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_range, probe.definition);
    }

    #[track_caller]
    fn last_publication(&self, uri: &Uri) -> Publication<'_> {
        let last = self
            .session
            .publications()
            .rev()
            .find(|publication| publication.uri() == uri);

        match last {
            Some(publication) => publication,
            None => panic!("Nothing was published for {}", uri.as_str()),
        }
    }
}

fn uri(path: &Path) -> Uri {
    Uri::from_file_path(path).unwrap()
}

fn range(start: (u32, u32), end: (u32, u32)) -> Range {
    Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))
}

/// Use the final edit's unique symbol to reject stale publications.
fn final_diagnostics(config: &BurstConfig) -> Vec<KeyFields> {
    let document = config.recurring(config.final_edit());
    let symbol = missing_symbol(config.final_edit());
    let line = document.code_line + MISSING_LINE;

    vec![(
        format!("No symbol named '{symbol}' in scope."),
        Some(DiagnosticSeverity::WARNING),
        range((line, 0), (line, symbol.len() as u32)),
    )]
}

/// Extract the fields asserted by this replay without pinning unrelated
/// protocol fields.
fn key_fields(diagnostics: &[Diagnostic]) -> Vec<KeyFields> {
    diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.message.clone(),
                diagnostic.severity,
                diagnostic.range,
            )
        })
        .collect()
}
