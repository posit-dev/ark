//! Resolve each environment's R libraries on the `oak-env` pool.
//!
//! An environment is a workspace root, an environment directory inside one
//! (holding `.Rprofile` or `.Renviron`), or the default for files outside
//! every root. Workspace environments get their library paths from a fresh R
//! subprocess started in their directory. The default runs in an empty temporary
//! directory. Neither uses the embedded R session's `.libPaths()`. The
//! `oak_scan` environment writers maintain the merged
//! [`LibraryRoots`](oak_db::LibraryRoots).
//!
//! Like workspace scans and source fetches, resolution jobs hold no database
//! handle while doing I/O. The main loop submits them to
//! [`LspState::environment_pool`]. Each job runs R and lists the packages in its
//! reported libraries. [`Event::EnvironmentResolved`] returns the result to
//! [`EnvironmentScheduler::finish()`] for a main-loop database write.
//!
//! [`LspState::environment_pool`]: crate::lsp::main_loop::LspState::environment_pool

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aether_path::FilePath;
use anyhow::anyhow;
use camino::Utf8Path;
use oak_db::DbInputs;
use oak_db::Environment;
use oak_db::OakDatabase;
use oak_r_process::Profile;
use oak_r_process::RunOptions;
use oak_scan::is_environment_sentinel;
use oak_scan::list_libraries;
use oak_scan::DbScan;
use oak_scan::LibraryListing;
use stdext::result::ResultExt;

use crate::lsp;
use crate::lsp::io_pool::IoPool;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;

/// Timeout passed to the R subprocess runner. A hung `.Rprofile` would
/// otherwise block the single `oak-env` thread and all queued resolutions.
pub(crate) const RESOLUTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Failure recorded by each resolution job when no R executable was found.
pub(crate) const NO_R_EXECUTABLE: &str = "no R executable was found";

/// Writes the subprocess's `.libPaths()` to the file named by the first trailing
/// argument, one UTF-8 path per line.
///
/// Only this script creates the result file, after R startup finishes. If a
/// startup file quits R before the script runs, the missing result is a failure
/// rather than a resolved environment with no libraries.
const LIBRARY_PATHS_SCRIPT: &str = r#"local({
    result <- commandArgs(trailingOnly = TRUE)[[1L]]
    writeLines(enc2utf8(.libPaths()), result, useBytes = TRUE)
})
"#;

/// Number of trailing `stderr` lines kept in the error of a failed run. R
/// reports the error that stopped startup last, followed by "Execution
/// halted".
const STDERR_TAIL_LINES: usize = 5;

/// Resolves an environment's library paths and lists their installed packages.
/// Tests can replace the R subprocess through this interface.
pub(crate) trait EnvironmentResolver: Send + Sync {
    /// Return libraries in `.libPaths()` lookup order, each with its package
    /// listing. On failure, the scheduler stores the error message in
    /// [`Environment::last_error()`] and logs it with the environment name.
    fn resolve(&self, request: &EnvironmentRequest) -> anyhow::Result<Vec<LibraryListing>>;
}

/// The R startup directory of one environment.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EnvironmentRequest {
    /// Run from an empty temporary directory to avoid workspace startup files.
    /// R still honors user and site startup files and environment overrides.
    Default,
    Directory(PathBuf),
}

/// Resolves libraries through a fresh R subprocess with normal startup-file
/// discovery, then lists the packages in each reported library.
pub(crate) struct RProcessResolver {
    r: PathBuf,
}

impl RProcessResolver {
    pub(crate) fn new(r: PathBuf) -> Self {
        Self { r }
    }

    /// The temporary directory owns the script and result file, plus the
    /// default environment's working directory. It is removed after the paths
    /// are read, before package listing begins.
    fn library_paths(&self, request: &EnvironmentRequest) -> anyhow::Result<Vec<PathBuf>> {
        let job_dir = tempfile::tempdir()
            .map_err(|err| anyhow!("can't create a temporary directory: {err}"))?;

        let script = job_dir.path().join("library-paths.R");
        fs::write(&script, LIBRARY_PATHS_SCRIPT)
            .map_err(|err| anyhow!("can't write the R script: {err}"))?;

        let result = job_dir.path().join("library-paths.txt");
        let Some(result_arg) = result.to_str() else {
            return Err(anyhow!(
                "the temporary directory {path} isn't valid UTF-8",
                path = job_dir.path().display()
            ));
        };

        let working_dir = match request {
            // Spawning in a missing directory fails with the same error as a
            // missing R executable, so name the directory instead.
            EnvironmentRequest::Directory(dir) if !dir.is_dir() => {
                return Err(anyhow!(
                    "the directory {dir} doesn't exist",
                    dir = dir.display()
                ));
            },
            EnvironmentRequest::Directory(dir) => dir.clone(),
            EnvironmentRequest::Default => {
                let dir = job_dir.path().join("default");
                fs::create_dir(&dir)
                    .map_err(|err| anyhow!("can't create a temporary directory: {err}"))?;
                dir
            },
        };

        let options = RunOptions {
            profile: Profile::Startup,
            working_dir: Some(&working_dir),
            timeout: Some(RESOLUTION_TIMEOUT),
            args: &[result_arg],
            env: &[],
        };

        // A timeout is one of these errors, and its message names the limit.
        let output = match oak_r_process::run_file(&self.r, &script, &options) {
            Ok(output) => output,
            Err(err) => return Err(anyhow!("can't run R at {r}: {err:#}", r = self.r.display())),
        };

        if !output.status.success() {
            let status = match output.status.code() {
                Some(code) => format!("R exited with status {code}"),
                None => String::from("R was terminated"),
            };
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(match stderr_tail(&stderr) {
                Some(tail) => anyhow!("{status}:\n{tail}"),
                None => anyhow!("{status}"),
            });
        }

        let contents = match fs::read_to_string(&result) {
            Ok(contents) => contents,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(anyhow!(
                    "R exited before reporting its library paths, a startup file may have quit R"
                ));
            },
            Err(err) => {
                return Err(anyhow!("can't read the library paths R reported: {err}"));
            },
        };

        Ok(parse_library_paths(&contents))
    }
}

impl EnvironmentResolver for RProcessResolver {
    fn resolve(&self, request: &EnvironmentRequest) -> anyhow::Result<Vec<LibraryListing>> {
        let paths = self.library_paths(request)?;
        Ok(list_libraries(paths))
    }
}

/// Create the production database with its default environment unresolved,
/// distinguishing pending resolution from a successful run with no libraries.
/// [`OakDatabase::new()`] instead starts with the default resolved to no libraries.
/// Tests and benchmarks can use that constructor without requiring R startup.
pub(crate) fn unresolved_database() -> OakDatabase {
    let mut db = OakDatabase::new();
    db.mark_default_environment_unresolved();
    db
}

fn parse_library_paths(contents: &str) -> Vec<PathBuf> {
    contents
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn stderr_tail(stderr: &str) -> Option<String> {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    Some(lines[start..].join("\n"))
}

/// Resolves every environment to the same library paths, listing packages from
/// disk on each run. The LSP harness uses this to preserve its configured
/// libraries through the production startup handshake without starting R.
#[cfg(any(test, feature = "testing"))]
pub(crate) struct FixedEnvironmentResolver {
    paths: Vec<PathBuf>,
}

#[cfg(any(test, feature = "testing"))]
impl FixedEnvironmentResolver {
    pub(crate) fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths }
    }
}

#[cfg(any(test, feature = "testing"))]
impl EnvironmentResolver for FixedEnvironmentResolver {
    fn resolve(&self, _request: &EnvironmentRequest) -> anyhow::Result<Vec<LibraryListing>> {
        Ok(list_libraries(self.paths.clone()))
    }
}

#[derive(Debug)]
pub(crate) struct EnvironmentCompleted {
    pub(crate) environment: Environment,
    pub(crate) result: anyhow::Result<Vec<LibraryListing>>,
}

/// State of an environment's job. Environments without a job have no entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobState {
    Running,

    /// A request arrived before completion. Discard the result and run again
    /// because it may not reflect the edit that triggered the request.
    RunningWithRerunQueued,
}

/// Decides which environments to resolve and when, and applies results
/// through `oak_scan`'s environment writers.
///
/// At startup, the one-thread pool runs the default first, then workspace
/// roots in workspace order. Scans add nested environments as they discover
/// them. Submission order does not determine the merged library order, which
/// the writers derive from the live environment list.
pub(crate) struct EnvironmentScheduler {
    /// `None` when no R executable was found. Every environment then fails
    /// with [`NO_R_EXECUTABLE`].
    resolver: Option<Arc<dyn EnvironmentResolver>>,

    /// Includes queued and running jobs, even for removed environments. Entries
    /// remain until completion so settlement waits for those jobs too.
    jobs: HashMap<Environment, JobState>,
}

impl EnvironmentScheduler {
    pub(crate) fn new(resolver: Option<Arc<dyn EnvironmentResolver>>) -> Self {
        Self {
            resolver,
            jobs: HashMap::new(),
        }
    }

    pub(crate) fn request_default(
        &mut self,
        db: &OakDatabase,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        let default = db.default_environment();
        self.request(db, [default], pool, events_tx);
    }

    /// Resolve newly added workspace environments. The writer removes
    /// environments whose directories are no longer wanted and evicts libraries
    /// that no remaining environment lists.
    pub(crate) fn reconcile(
        &mut self,
        db: &mut OakDatabase,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        let added = db.reconcile_environments();
        self.request(db, added, pool, events_tx);
    }

    /// Request one run per live workspace environment whose directory contains
    /// a `.Rprofile` or `.Renviron` path in the watcher batch.
    ///
    /// Directories without a live environment must first be discovered by a
    /// workspace scan, then added through [`Self::reconcile()`].
    pub(crate) fn rerun_for_sentinels(
        &mut self,
        db: &OakDatabase,
        paths: &[FilePath],
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        let sentinel_dirs: Vec<&Utf8Path> = paths
            .iter()
            .filter_map(|path| path.as_path())
            .filter(|path| path.file_name().is_some_and(is_environment_sentinel))
            .filter_map(|path| path.parent())
            .collect();
        if sentinel_dirs.is_empty() {
            return;
        }

        let environments: Vec<Environment> = db
            .workspace_environments()
            .environments(db)
            .iter()
            .copied()
            .filter(|environment| {
                environment
                    .dir(db)
                    .as_ref()
                    .and_then(|dir| dir.as_path())
                    .is_some_and(|dir| sentinel_dirs.contains(&dir))
            })
            .collect();
        self.request(db, environments, pool, events_tx);
    }

    /// Coalesce requests while a job is queued or running. Discard that job
    /// result and submit one more run on completion, regardless of how many
    /// requests arrive in the meantime.
    fn request(
        &mut self,
        db: &OakDatabase,
        environments: impl IntoIterator<Item = Environment>,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        for environment in environments {
            match self.jobs.get_mut(&environment) {
                Some(state) => *state = JobState::RunningWithRerunQueued,
                None => self.submit(db, environment, pool, events_tx),
            }
        }
    }

    fn submit(
        &mut self,
        db: &OakDatabase,
        environment: Environment,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        // A directory that isn't a file path fails through the job, so the
        // failure is recorded and explained like any other.
        let request = match environment.dir(db) {
            None => Ok(EnvironmentRequest::Default),
            Some(dir) => match dir.as_path() {
                Some(path) => Ok(EnvironmentRequest::Directory(
                    path.as_std_path().to_path_buf(),
                )),
                None => Err(anyhow!("{dir} is not a file path")),
            },
        };

        self.jobs.insert(environment, JobState::Running);

        let resolver = self.resolver.as_ref().map(Arc::clone);
        let guard = ResolutionFailureGuard {
            environment,
            events_tx: Some(events_tx.clone()),
        };
        pool.submit(move || {
            let result = match (&resolver, request) {
                (_, Err(err)) => Err(err),
                (Some(resolver), Ok(request)) => resolver.resolve(&request),
                (None, Ok(_)) => Err(anyhow!(NO_R_EXECUTABLE)),
            };
            guard.complete(result);
        });
    }

    /// Apply results only for live environments with no queued rerun. A removed
    /// environment gets no further run. A live environment with a queued rerun
    /// runs again without applying this result.
    ///
    /// A success replaces the environment's libraries. A failure records its
    /// error, preserving the libraries from its last successful run, if any.
    pub(crate) fn finish(
        &mut self,
        db: &mut OakDatabase,
        completed: EnvironmentCompleted,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        let EnvironmentCompleted {
            environment,
            result,
        } = completed;

        let state = self.jobs.remove(&environment);

        if !environment.is_live(db) {
            return;
        }

        match state {
            Some(JobState::RunningWithRerunQueued) => {
                self.submit(db, environment, pool, events_tx);
                return;
            },
            Some(JobState::Running) => {},
            None => log::warn!("Received an untracked environment resolution, applying it"),
        }

        let name = environment_name(db, environment);
        match result {
            Ok(libraries) => {
                let paths: Vec<String> = libraries
                    .iter()
                    .map(|library| library.path.display().to_string())
                    .collect();
                lsp::log_info!(
                    "Resolved R libraries for {name}: {paths}",
                    paths = paths.join(", ")
                );
                db.resolve_environment(environment, libraries);
            },
            Err(err) => {
                let error = format!("{err:#}");
                log::warn!("Can't resolve R libraries for {name}: {error}");
                db.record_environment_failure(environment, error);
            },
        }
    }

    /// Whether a run for `environment` is queued or running, so its libraries,
    /// and the packages they hold, can still change. Checks the job rather
    /// than [`Environment::status()`], so an environment whose run failed
    /// counts as settled until a rerun is requested.
    pub(crate) fn is_pending(&self, environment: Environment) -> bool {
        self.jobs.contains_key(&environment)
    }

    /// Resolution jobs are tracked outside the analysis pool counters, so
    /// settlement must check them separately.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn has_pending(&self) -> bool {
        !self.jobs.is_empty()
    }
}

fn environment_name(db: &OakDatabase, environment: Environment) -> String {
    match environment.dir(db) {
        Some(dir) => dir.to_string(),
        None => String::from("the default environment"),
    }
}

/// Sends a completion even if the resolver panics or the pool drops the job
/// without running it, so the scheduler can clear its pending entry.
/// [`Self::complete()`] sends the normal result and disarms the guard.
struct ResolutionFailureGuard {
    environment: Environment,
    events_tx: Option<TokioUnboundedSender<Event>>,
}

impl ResolutionFailureGuard {
    fn complete(mut self, result: anyhow::Result<Vec<LibraryListing>>) {
        let Some(events_tx) = self.events_tx.take() else {
            return;
        };
        events_tx
            .send(Event::EnvironmentResolved(EnvironmentCompleted {
                environment: self.environment,
                result,
            }))
            .log_err();
    }
}

impl Drop for ResolutionFailureGuard {
    fn drop(&mut self) {
        let Some(events_tx) = self.events_tx.take() else {
            return;
        };
        // Log with `log_err()`, not `lsp::log_error!`: the latter goes through
        // the auxiliary channel, which can panic, and a panic in `drop()`
        // while the job is already unwinding aborts the process.
        events_tx
            .send(Event::EnvironmentResolved(EnvironmentCompleted {
                environment: self.environment,
                result: Err(anyhow!(
                    "the library resolution job stopped before reporting a result"
                )),
            }))
            .log_err();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::parse_library_paths;
    use super::stderr_tail;

    #[test]
    fn test_parse_library_paths_skips_empty_lines() {
        let paths = parse_library_paths("/lib/a\n\n/lib/b\r\n");
        assert_eq!(paths, vec![
            PathBuf::from("/lib/a"),
            PathBuf::from("/lib/b")
        ]);
    }

    #[test]
    fn test_stderr_tail_keeps_last_lines() {
        let stderr = "1\n2\n\n3\n4\n5\n6\n";
        assert_eq!(stderr_tail(stderr), Some(String::from("2\n3\n4\n5\n6")));
        assert_eq!(stderr_tail("\n  \n"), None);
    }
}
