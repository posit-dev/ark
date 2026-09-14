//! Records the scenario running in each fuzz process.
//!
//! Nextest kills timed-out processes without unwinding, so each scenario and
//! operation is written before it runs. Successful checks clear the file.

use std::cell::RefCell;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Seek;
use std::io::SeekFrom;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

pub(crate) struct Artifact {
    path: PathBuf,
    file: RefCell<File>,
    /// Cached scenario text avoids rendering it for every operation.
    header: RefCell<String>,
}

impl Artifact {
    /// Print the artifact path immediately so CI timeout logs can identify it.
    pub(crate) fn open() -> Artifact {
        let path = artifact_path();
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                panic!("failed to create {}: {err}", parent.display());
            }
        }
        let file = match OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
        {
            Ok(file) => file,
            Err(err) => panic!("failed to open fuzz artifact {}: {err}", path.display()),
        };
        eprintln!("fuzz artifact: {}", path.display());
        Artifact {
            path,
            file: RefCell::new(file),
            header: RefCell::new(String::new()),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn reset(&self, header: String) {
        self.write(&header);
        *self.header.borrow_mut() = header;
    }

    pub(crate) fn entering(&self, operation: &str) {
        let header = self.header.borrow();
        self.write(&format!("{header}  current: {operation}\n"));
    }

    pub(crate) fn clear(&self) {
        self.write("");
        self.header.borrow_mut().clear();
    }

    fn write(&self, content: &str) {
        let mut file = self.file.borrow_mut();
        if let Err(err) = write_in_place(&mut file, content) {
            eprintln!(
                "fuzz artifact write to {} failed: {err}",
                self.path.display()
            );
        }
    }
}

fn write_in_place(file: &mut File, content: &str) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(content.as_bytes())?;
    file.set_len(content.len() as u64)
}

/// An external driver's worker threads are typically all named `main`, so the
/// process id keeps their artifact paths from colliding.
fn artifact_path() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/oak_fuzz");
    let pid = std::process::id();
    let name = match std::thread::current().name() {
        Some(name) => format!("{}-{pid}", name.replace("::", "_")),
        None => format!("pid-{pid}"),
    };
    dir.join(format!("{name}.artifact"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_path_includes_process_id() {
        let path = artifact_path();
        let file_name = path.file_name().unwrap().to_str().unwrap();
        assert!(file_name.contains(&std::process::id().to_string()));
    }
}
