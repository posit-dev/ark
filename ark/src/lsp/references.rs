// 
// references.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use std::path::Path;

use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::Range;
use tower_lsp::lsp_types::ReferenceParams;
use tower_lsp::lsp_types::Url;
use tree_sitter::Point;
use walkdir::DirEntry;
use walkdir::WalkDir;

use crate::lsp::backend::Backend;
use crate::lsp::document::Document;
use crate::lsp::logger::log_push;
use crate::lsp::macros::unwrap;
use crate::lsp::traits::cursor::TreeCursorExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::position::PositionExt;

fn _filter_entry(entry: &DirEntry) -> bool {

    // TODO: Figure out if we can read this from the front-end;
    // the user has likely defined a set of workspace file filters
    // that could control which files we search for references in.
    let name = entry.file_name().to_str().unwrap_or("");
    match name {
        ".git" | "node_modules" => false,
        _ => true,
    }

}

impl Backend {

    fn find_identifier_at_point(&self, uri: &Url, point: Point) -> Result<String, ()> {

        // Unwrap the URL.
        let path = unwrap!(uri.to_file_path(), {
            log_push!("URL {} not associated with a local file path", uri);
            return Err(());
        });

        // Figure out the identifier we're looking for.
        let identifier = self.with_document(path.as_path(), |document| {
        
            let ast = unwrap!(document.ast.as_ref(), {
                log_push!("no ast associated with document {}", path.display());
                return Err(());
            });

            let mut node = unwrap!(ast.root_node().descendant_for_point_range(point, point), {
                log_push!("couldn't find node associated with point {:?}", point);
                return Err(());
            });

            // Check and see if we got an identifier. If we didn't, we might need to use
            // some heuristics to look around. Unfortunately, it seems like if you double-click
            // to select an identifier, and then use Right Click -> Find All References, the
            // position received by the LSP maps to the _end_ of the selected range, which
            // is technically not part of the associated identifier's range. In addition, we
            // can't just subtract 1 from the position column since that would then fail to
            // resolve the correct identifier when the cursor is located at the start of the
            // identifier.
            if node.kind() != "identifier" {
                let point = Point::new(point.row, point.column - 1);
                node = unwrap!(ast.root_node().descendant_for_point_range(point, point), {
                    log_push!("couldn't find node associated with point {:?}", point);
                    return Err(());
                });
            }

            // double check that we found an identifier
            if node.kind() != "identifier" {
                log_push!("couldn't find an identifier associated with point {:?}", point);
                return Err(());
            }

            // return identifier text contents
            let contents = document.contents.to_string();
            let needle = node.utf8_text(contents.as_bytes()).expect("node contents");
            return Ok(needle.to_string());

        });

        return identifier;

    }

    fn find_references_in_folder(&self, identifier: &str, path: &Path, locations: &mut Vec<Location>) {
        
        let walker = WalkDir::new(path);
        for entry in walker.into_iter().filter_entry(|entry| _filter_entry(entry)) {

            let entry = unwrap!(entry, { continue; });
            let path = entry.path();
            let ext = unwrap!(path.extension(), { continue; });
            if ext != "r" && ext != "R" { continue; }

            log_push!("found R file {}", path.display());
            let result = self.with_document(path, |document| {
                self.find_references_in_document(identifier, path, document, locations);
                return Ok(());
            });

            match result {
                Ok(result) => result,
                Err(_error) => {
                    log_push!("error retrieving document for path {}", path.display());
                    continue;
                }
            }

        }

    }

    fn find_references_in_document(&self, identifier: &str, path: &Path, document: &Document, locations: &mut Vec<Location>) {
        
        // recurse and find symbols of the matching name
        let ast = unwrap!(document.ast.as_ref(), {
            log_push!("no ast available");
            return;
        });

        let contents = document.contents.to_string();

        let mut cursor = ast.walk();
        cursor.recurse(|node| {

            if node.kind() == "identifier" {
                let text = node.utf8_text(contents.as_bytes()).expect("contents");
                if text == identifier {
                    let location = Location::new(
                        Url::from_file_path(path).expect("valid path"),
                        Range::new(node.start_position().as_position(), node.end_position().as_position())
                    );
                    locations.push(location);
                }
            }

            return true;

        });

    }

    pub(crate) fn find_references(&self, params: ReferenceParams) -> Result<Vec<Location>, ()> {

        // Create our locations vector.
        let mut locations : Vec<Location> = Vec::new();

        // Extract relevant parameters.
        let uri = params.text_document_position.text_document.uri;
        let point = params.text_document_position.position.as_point();
        
        // Figure out the identifier we're looking for.
        let identifier = match self.find_identifier_at_point(&uri, point) {
            Ok(identifier) => identifier,
            Err(_error) => {
                log_push!("failed to find identifier at point {}", point);
                return Err(());
            }
        };

        // Now, start searching through workspace folders for references to that identifier.
        if let Ok(workspace) = self.workspace.lock() {
            for folder in workspace.folders.iter() {
                if let Ok(path) = folder.to_file_path() {
                    log_push!("searching references in folder {}", path.display());
                    self.find_references_in_folder(&identifier, &path, &mut locations);
                }
            }
        }


        return Ok(locations);
        
    }
}
