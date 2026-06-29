use std::path::Path;

/// Assembles a `NAMESPACE` file
#[derive(Default)]
pub(crate) struct NamespaceWriter {
    directives: Vec<String>,
}

impl NamespaceWriter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn export(self, name: &str) -> Self {
        self.directive(format!("export({name})"))
    }

    pub(crate) fn import(self, package: &str) -> Self {
        self.directive(format!("import({package})"))
    }

    pub(crate) fn import_from(self, package: &str, name: &str) -> Self {
        self.directive(format!("importFrom({package}, {name})"))
    }

    fn directive(mut self, directive: String) -> Self {
        self.directives.push(directive);
        self
    }

    /// Write the assembled `NAMESPACE` into `dir`
    pub(crate) fn write(self, dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let contents: String = self
            .directives
            .iter()
            .map(|directive| format!("{directive}\n"))
            .collect();
        std::fs::write(dir.join("NAMESPACE"), contents).unwrap();
    }
}
