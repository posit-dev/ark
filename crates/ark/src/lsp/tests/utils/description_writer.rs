use std::path::Path;

/// Assembles a `DESCRIPTION` file from parts
#[derive(Default)]
pub(crate) struct DescriptionWriter {
    fields: Vec<(String, String)>,
}

impl DescriptionWriter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn package(self, name: &str) -> Self {
        self.field("Package", name)
    }

    pub(crate) fn version(self, version: &str) -> Self {
        self.field("Version", version)
    }

    pub(crate) fn built(self, built: &str) -> Self {
        self.field("Built", built)
    }

    pub(crate) fn imports(self, imports: &[&str]) -> Self {
        self.field("Imports", &imports.join(", "))
    }

    pub(crate) fn field(mut self, key: &str, value: &str) -> Self {
        self.fields.push((key.to_string(), value.to_string()));
        self
    }

    /// Write the assembled `DESCRIPTION` into `dir`.
    pub(crate) fn write(self, dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let contents: String = self
            .fields
            .iter()
            .map(|(key, value)| format!("{key}: {value}\n"))
            .collect();
        std::fs::write(dir.join("DESCRIPTION"), contents).unwrap();
    }
}
