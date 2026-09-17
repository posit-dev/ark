//! Chooses source arguments relative to the file containing the call.
//!
//! `anchor_dir()` resolves relative paths against that file's workspace or
//! package root, not its own directory. Candidate pools are therefore keyed on
//! [`Owner`]. Paths owned only by another root remain available as deliberate
//! unresolvable draws.
//!
//! `source()` accepts files, `sourceDir()` accepts directories, and
//! `tar_source()` accepts either.

use oak_semantic::effects::fuzz::SourceProvider;

use crate::fuzz::choose::Choose;
use crate::fuzz::spec::Owner;
use crate::fuzz::spec::WorkspaceSpec;

/// Keep missing paths, mismatched kinds, and foreign roots reachable without
/// letting them dominate.
const UNRESOLVABLE_PERCENT: u32 = 20;

const MISSING_FILE: &str = "missing.R";
const MISSING_DIR: &str = "missing";

/// Candidate paths relative to one owner's root, partitioned by path kind.
pub(super) struct SourceCandidates {
    files: Vec<String>,
    /// Always holds `.`, the owner's own root.
    dirs: Vec<String>,
    /// Paths that resolve only against a different root.
    foreign: Vec<String>,
}

impl SourceCandidates {
    pub(super) fn for_owner(spec: &WorkspaceSpec, owner: Owner) -> SourceCandidates {
        let mut files = Vec::new();
        let mut dirs = vec![".".to_string()];
        let mut foreign = Vec::new();

        for file in &spec.files {
            if file.owner != owner {
                foreign.push(file.path.clone());
                continue;
            }
            files.push(file.path.clone());
            for dir in ancestor_dirs(&file.path) {
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }

        // Two packages can each own `R/a.R`, and that spelling resolves locally.
        foreign.retain(|path| !files.contains(path));

        SourceCandidates {
            files,
            dirs,
            foreign,
        }
    }

    /// Usually returns a path that `provider` can resolve.
    pub(super) fn target(&self, rng: &mut impl Choose, provider: SourceProvider) -> String {
        if rng.odds(UNRESOLVABLE_PERCENT) {
            return self.unresolvable(rng, provider);
        }
        self.resolvable(rng, provider)
    }

    /// Returns a path other than `current`, or `None` when none is available.
    pub(super) fn redirect(
        &self,
        rng: &mut impl Choose,
        provider: SourceProvider,
        current: &str,
    ) -> Option<String> {
        let candidate = self.target(rng, provider);
        if candidate != current {
            return Some(candidate);
        }
        let others: Vec<&String> = self
            .pool(rng, provider)
            .iter()
            .filter(|path| path.as_str() != current)
            .collect();
        if !others.is_empty() {
            return Some(others[rng.index(others.len())].clone());
        }

        // Returning `None` would report a successful mutation without changing
        // a lone self-source. Prefer the provider's missing-path kind; the other
        // kind guarantees a change when `current` already equals that fallback.
        let missing = match provider {
            SourceProvider::Dir => [MISSING_DIR, MISSING_FILE],
            SourceProvider::File | SourceProvider::FileOrDir => [MISSING_FILE, MISSING_DIR],
        };
        missing
            .into_iter()
            .find(|candidate| *candidate != current)
            .map(str::to_string)
    }

    /// Reports whether `path` is local and has a kind accepted by `provider`.
    pub(super) fn accepts(&self, provider: SourceProvider, path: &str) -> bool {
        let owned = |pool: &[String]| pool.iter().any(|candidate| candidate == path);
        match provider {
            SourceProvider::File => owned(&self.files),
            SourceProvider::Dir => owned(&self.dirs),
            SourceProvider::FileOrDir => owned(&self.files) || owned(&self.dirs),
        }
    }

    pub(super) fn resolvable(&self, rng: &mut impl Choose, provider: SourceProvider) -> String {
        let pool = self.pool(rng, provider);
        // `dirs` always contains `.`, and `files` contains the calling file.
        pool[rng.index(pool.len())].clone()
    }

    fn pool(&self, rng: &mut impl Choose, provider: SourceProvider) -> &Vec<String> {
        match provider {
            SourceProvider::File => &self.files,
            SourceProvider::Dir => &self.dirs,
            SourceProvider::FileOrDir => {
                if rng.odds(50) {
                    &self.files
                } else {
                    &self.dirs
                }
            },
        }
    }

    /// `FileOrDir` needs a missing or foreign path because either local kind
    /// would resolve.
    fn unresolvable(&self, rng: &mut impl Choose, provider: SourceProvider) -> String {
        match rng.index(3) {
            0 if !self.foreign.is_empty() => self.foreign[rng.index(self.foreign.len())].clone(),
            1 => match provider {
                SourceProvider::File => self.dirs[rng.index(self.dirs.len())].clone(),
                SourceProvider::Dir => self.files[rng.index(self.files.len())].clone(),
                SourceProvider::FileOrDir => MISSING_DIR.to_string(),
            },
            _ => match provider {
                SourceProvider::Dir => MISSING_DIR.to_string(),
                SourceProvider::File | SourceProvider::FileOrDir => MISSING_FILE.to_string(),
            },
        }
    }
}

/// Every directory between `path`'s parent and the owner's root, nearest first.
/// The root itself is spelled `.` and is added separately.
fn ancestor_dirs(path: &str) -> Vec<String> {
    let mut segments: Vec<&str> = path.split('/').collect();
    segments.pop();

    let mut out = Vec::new();
    while !segments.is_empty() {
        out.push(segments.join("/"));
        segments.pop();
    }
    out
}
