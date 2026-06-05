use std::collections::HashSet;

use biome_rowan::TextSize;
use oak_db::all_files;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Identifier;
use oak_db::MemberKind;
use oak_semantic::ScopeId;

use crate::FileRange;

/// Find all references to the symbol at `offset` in `file`.
///
/// Uses `resolve_at()` to confirm each candidate: a textual mention of the
/// same name is only included when it resolves to the same definition set.
/// Member-name cursors (`$`/`@` RHS) fall back to a structural cross-file scan.
pub fn find_references(
    db: &dyn Db,
    file: File,
    offset: TextSize,
    include_declaration: bool,
) -> Vec<FileRange> {
    let Some(ident) = Identifier::classify(db, file, offset) else {
        return Vec::new();
    };

    match ident {
        Identifier::Variable { range, .. } => {
            find_variable_references(db, file, range.start(), include_declaration)
        },
        Identifier::Member { name, kind, .. } => {
            find_member_references(db, file, name.text(db).as_str(), kind)
        },
    }
}

fn find_variable_references(
    db: &dyn Db,
    file: File,
    snapped: TextSize,
    include_declaration: bool,
) -> Vec<FileRange> {
    let target_defs = file.resolve_at(db, snapped);
    if target_defs.is_empty() {
        return Vec::new();
    }

    let name = target_defs[0].name(db);

    let file_scope = ScopeId::from(0);
    let locally_scoped = target_defs.iter().all(|d| d.scope(db) != file_scope);

    let files = if locally_scoped {
        vec![file]
    } else {
        all_matching_files(db, name.text(db).as_str())
    };

    // Rust-Analyzer does a pure text search across all files, then resolves
    // each occurrences. We are more aligned with ty: we filter files by a text
    // search, but then we walk a post-parse tree. ty walks a raw AST, we walk
    // the index via `uses_of()`. The latter is more to the point.

    // A candidate is a reference when it resolves to the same binding as the cursor.
    let target_set: HashSet<Definition<'_>> = target_defs.iter().copied().collect();

    let mut results = Vec::new();

    for file in files {
        for range in file.uses_of(db, name) {
            let candidate_defs = file.resolve_at(db, range.start());
            if candidate_defs.iter().any(|d| target_set.contains(d)) {
                results.push(FileRange { file, range });
            }
        }
    }

    if include_declaration {
        for def in &target_defs {
            if let Some(range) = def.name_range(db) {
                results.push(FileRange {
                    file: def.file(db),
                    range,
                });
            }
        }
    }

    sort_file_ranges(&mut results, db, file);
    results
}

fn find_member_references(
    db: &dyn Db,
    primary: File,
    name: &str,
    kind: MemberKind,
) -> Vec<FileRange> {
    let mut results = Vec::new();

    for file in all_matching_files(db, name) {
        for range in file.member_uses(db, name, kind) {
            results.push(FileRange { file, range });
        }
    }

    sort_file_ranges(&mut results, db, primary);
    results
}

/// Every db file whose contents mention `text`.
fn all_matching_files(db: &dyn Db, text: &str) -> Vec<File> {
    all_files(db)
        .iter()
        .filter(|&&f| f.contents(db).contains(text))
        .copied()
        .collect()
}

/// Sort current file first, then other files alphabetically by URL, with
/// source order within each file. Deduplicates identical (file, range) pairs.
fn sort_file_ranges(ranges: &mut Vec<FileRange>, db: &dyn Db, primary: File) {
    ranges.sort_by_cached_key(|r| {
        (
            r.file != primary,
            r.file.url(db).to_url().to_string(),
            r.range.start(),
        )
    });
    ranges.dedup_by(|a, b| a.file == b.file && a.range == b.range);
}
