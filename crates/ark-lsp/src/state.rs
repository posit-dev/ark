//
// state.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use ropey::Rope;
use tower_lsp::lsp_types::TextDocumentContentChangeEvent;
use tower_lsp::lsp_types::Url;
use tree_sitter::Parser;
use tree_sitter::Tree;

/// A parsed document
pub struct Document {
    pub contents: Rope,
    pub tree: Option<Tree>,
}

impl Document {
    pub fn new(text: &str) -> Self {
        let contents = Rope::from_str(text);
        let tree = parse_r(&contents);
        Self { contents, tree }
    }

    pub fn apply_change(&mut self, change: TextDocumentContentChangeEvent) {
        if let Some(range) = change.range {
            let start_line = range.start.line as usize;
            let start_char = range.start.character as usize;
            let end_line = range.end.line as usize;
            let end_char = range.end.character as usize;

            let start_idx = self.contents.line_to_char(start_line) + start_char;
            let end_idx = self.contents.line_to_char(end_line) + end_char;

            self.contents.remove(start_idx..end_idx);
            self.contents.insert(start_idx, &change.text);
        } else {
            // Full document sync
            self.contents = Rope::from_str(&change.text);
        }

        self.tree = parse_r(&self.contents);
    }

    pub fn text(&self) -> String {
        self.contents.to_string()
    }
}

fn parse_r(contents: &Rope) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_r::LANGUAGE.into())
        .ok()?;
    let text = contents.to_string();
    parser.parse(&text, None)
}

/// Package metadata loaded from disk
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Package {
    pub name: String,
    pub path: PathBuf,
    pub exports: Vec<String>,
    pub description: Option<String>,
    pub version: Option<String>,
}

impl Package {
    #[allow(dead_code)]
    pub fn load(path: &PathBuf) -> Option<Self> {
        let description_path = path.join("DESCRIPTION");
        if !description_path.exists() {
            return None;
        }

        let description_text = fs::read_to_string(&description_path).ok()?;
        let name = parse_dcf_field(&description_text, "Package")?;
        let version = parse_dcf_field(&description_text, "Version");
        let title = parse_dcf_field(&description_text, "Title");

        // Parse NAMESPACE for exports
        let exports = parse_namespace_exports(&path.join("NAMESPACE"));

        // Also include symbols from INDEX file (for datasets)
        let mut all_exports = exports;
        if let Some(index_exports) = parse_index(&path.join("INDEX")) {
            for sym in index_exports {
                if !all_exports.contains(&sym) {
                    all_exports.push(sym);
                }
            }
        }

        Some(Self {
            name,
            path: path.clone(),
            exports: all_exports,
            description: title,
            version,
        })
    }
}

#[allow(dead_code)]
fn parse_dcf_field(text: &str, field: &str) -> Option<String> {
    for line in text.lines() {
        if line.starts_with(field) && line.contains(':') {
            let value = line.splitn(2, ':').nth(1)?.trim();
            return Some(value.to_string());
        }
    }
    None
}

#[allow(dead_code)]
fn parse_namespace_exports(path: &PathBuf) -> Vec<String> {
    let mut exports = Vec::new();

    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return exports,
    };

    // Simple regex-free parsing of NAMESPACE export directives
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("export(") {
            // export(foo, bar, baz)
            if let Some(args) = line.strip_prefix("export(").and_then(|s| s.strip_suffix(')')) {
                for arg in args.split(',') {
                    let sym = arg.trim().trim_matches('"');
                    if !sym.is_empty() {
                        exports.push(sym.to_string());
                    }
                }
            }
        } else if line.starts_with("exportPattern(") {
            // We can't expand patterns without R, skip
        } else if line.starts_with("S3method(") {
            // S3method(print, foo) exports print.foo
            if let Some(args) = line.strip_prefix("S3method(").and_then(|s| s.strip_suffix(')')) {
                let parts: Vec<&str> = args.split(',').map(|s| s.trim()).collect();
                if parts.len() >= 2 {
                    let method = format!("{}.{}", parts[0], parts[1]);
                    exports.push(method);
                }
            }
        }
    }

    exports
}

#[allow(dead_code)]
fn parse_index(path: &PathBuf) -> Option<Vec<String>> {
    let text = fs::read_to_string(path).ok()?;
    let mut symbols = Vec::new();

    for line in text.lines() {
        // INDEX format: symbol_name<whitespace>description
        if let Some(sym) = line.split_whitespace().next() {
            if !sym.is_empty() && sym.chars().next().map(|c| c.is_alphabetic()).unwrap_or(false) {
                symbols.push(sym.to_string());
            }
        }
    }

    Some(symbols)
}

/// Library of installed packages
#[allow(dead_code)]
pub struct Library {
    paths: Vec<PathBuf>,
    packages: HashMap<String, Arc<Package>>,
}

impl Library {
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
            packages: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    pub fn get(&mut self, name: &str) -> Option<Arc<Package>> {
        if let Some(pkg) = self.packages.get(name) {
            return Some(pkg.clone());
        }

        // Try to load from library paths
        for lib_path in &self.paths {
            let pkg_path = lib_path.join(name);
            if let Some(pkg) = Package::load(&pkg_path) {
                let pkg = Arc::new(pkg);
                self.packages.insert(name.to_string(), pkg.clone());
                return Some(pkg);
            }
        }

        None
    }

    /// List all installed package names
    #[allow(dead_code)]
    pub fn list_packages(&self) -> Vec<String> {
        let mut names = Vec::new();
        for lib_path in &self.paths {
            if let Ok(entries) = fs::read_dir(lib_path) {
                for entry in entries.flatten() {
                    if entry.path().join("DESCRIPTION").exists() {
                        if let Some(name) = entry.file_name().to_str() {
                            if !names.contains(&name.to_string()) {
                                names.push(name.to_string());
                            }
                        }
                    }
                }
            }
        }
        names.sort();
        names
    }
}

/// Global LSP state
pub struct WorldState {
    pub documents: HashMap<Url, Document>,
    pub workspace_folders: Vec<Url>,
    pub library: Library,
}

impl WorldState {
    pub fn new(library_paths: Vec<PathBuf>) -> Self {
        Self {
            documents: HashMap::new(),
            workspace_folders: Vec::new(),
            library: Library::new(library_paths),
        }
    }

    pub fn open_document(&mut self, uri: Url, text: &str) {
        self.documents.insert(uri, Document::new(text));
    }

    pub fn close_document(&mut self, uri: &Url) {
        self.documents.remove(uri);
    }

    pub fn apply_change(&mut self, uri: &Url, change: TextDocumentContentChangeEvent) {
        if let Some(doc) = self.documents.get_mut(uri) {
            doc.apply_change(change);
        }
    }

    pub fn get_document(&self, uri: &Url) -> Option<&Document> {
        self.documents.get(uri)
    }
}
