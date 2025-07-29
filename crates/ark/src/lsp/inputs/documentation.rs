use std::path::Path;

use crate::lsp::inputs::documentation_rd_file::RdFile;

#[derive(Default, Clone, Debug)]
pub struct Documentation {
    pub rd_files: Vec<RdFile>,
}

impl Documentation {
    /// Load .Rd files from the man directory
    pub fn load_from_folder(path: &Path) -> anyhow::Result<Self> {
        if !path.is_dir() {
            return Ok(Documentation::default());
        }

        let mut rd_files = Vec::new();

        for entry in std::fs::read_dir(&path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("Rd") {
                rd_files.push(RdFile::load_from_file(&path)?);
            }
        }

        Ok(Documentation { rd_files })
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;

    use tempfile::tempdir;

    use super::*;
    use crate::lsp::inputs::documentation_rd_file::RdDocType;

    fn create_rd_file(dir: &std::path::Path, name: &str, content: &str) {
        let file_path = dir.join(name);
        let mut file = File::create(&file_path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn test_load_from_folder_with_rd_files() {
        let dir = tempdir().unwrap();
        create_rd_file(dir.path(), "foo.Rd", "\\name{foo}\n\\docType{data}");
        create_rd_file(dir.path(), "bar.Rd", "\\name{bar}\n\\docType{package}");
        create_rd_file(dir.path(), "baz.Rd", "\\name{baz}\n% Some Rd file");
        create_rd_file(dir.path(), "qux.txt", "Not an Rd file");

        let documentation = Documentation::load_from_folder(dir.path()).unwrap();
        assert_eq!(documentation.rd_files.len(), 3);

        let doc_types: Vec<(Option<String>, Option<RdDocType>)> = documentation
            .rd_files
            .into_iter()
            .map(|rd| (rd.name, rd.doc_type))
            .collect();

        assert!(doc_types.contains(&(Some(String::from("foo")), Some(RdDocType::Data))));
        assert!(doc_types.contains(&(Some(String::from("bar")), Some(RdDocType::Package))));
        assert!(doc_types.contains(&(Some(String::from("baz")), None)));
    }

    #[test]
    fn test_load_from_folder_empty_or_nonexistent() {
        let dir = tempdir().unwrap();

        // No files in directory
        let documentation = Documentation::load_from_folder(dir.path()).unwrap();
        assert_eq!(documentation.rd_files.len(), 0);

        // Nonexistent directory
        let nonexistent = dir.path().join("does_not_exist");
        let documentation = Documentation::load_from_folder(&nonexistent).unwrap();
        assert_eq!(documentation.rd_files.len(), 0);
    }
}
