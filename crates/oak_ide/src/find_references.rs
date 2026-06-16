use std::collections::HashSet;

use biome_rowan::TextSize;
use oak_db::all_files;
use oak_db::Db;
use oak_db::Definition;
use oak_db::File;
use oak_db::Identifier;
use oak_db::MemberKind;
use oak_db::Name;
use oak_semantic::ScopeId;

use crate::FileRange;

/// Find all references to the symbol at `offset` in `file`.
///
/// Uses `resolve_at()` to confirm each candidate: a textual mention of the
/// same name is only included when it resolves to the same definition set.
/// Member-name cursors (`$`/`@` RHS) and namespace-access cursors (`pkg::sym`
/// RHS) fall back to a structural cross-file scan.
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
        Identifier::NamespaceAccess {
            namespace, name, ..
        } => {
            // TODO(namespace-refs): also union the bare-name references, the
            // inverse of the bridge in `find_variable_references`. When
            // `namespace` is a workspace package, resolve `name` to its
            // definition and run the variable path. Installed-package symbols
            // wait on package resolution (see `find_namespace_references`).
            find_namespace_references(
                db,
                file,
                namespace.text(db).as_str(),
                name.text(db).as_str(),
            )
        },
    }
}

fn find_variable_references(
    db: &dyn Db,
    file: File,
    offset: TextSize,
    include_declaration: bool,
) -> Vec<FileRange> {
    let target_defs = file.resolve_at(db, offset);
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

    // Bare `foo` and `pkg::foo` name the same binding when `foo` is a
    // package-level definition, so include the qualified sites too. Locally
    // scoped symbols (params, locals) can't be reached through `::`.
    if !locally_scoped {
        collect_package_qualified_uses(db, &target_defs, name, &mut results);
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

/// Add `pkg::name` / `pkg:::name` sites for a package-level binding.
///
/// Bare `name` and `pkg::name` are the same symbol when `name` is defined in
/// package `pkg`, so a reference search from the bare name should surface the
/// qualified sites too. We take the symbol's owning package from the target
/// definitions and scan structurally. The `pkg::` qualifier is itself the
/// confirmation that it's the right symbol, so unlike the bare-name path this
/// needs no re-resolution.
///
/// TODO(namespace-refs): the reverse (cursor on `pkg::name` also finding bare
/// `name`) is unimplemented in the `NamespaceAccess` arm of `find_references`.
/// Installed-package symbols bridge in neither direction until package
/// resolution lands (see `find_namespace_references`).
fn collect_package_qualified_uses<'db>(
    db: &'db dyn Db,
    target_defs: &[Definition<'db>],
    name: Name<'db>,
    results: &mut Vec<FileRange>,
) {
    // The cursor resolves to one binding, so its definitions all share a single
    // owning package. Find the first one.
    let Some(package) = target_defs.iter().find_map(|def| def.file(db).package(db)) else {
        // The symbol resolves to a definition in a script, there can't be
        // namespace references to it
        return;
    };
    let package = package.name(db);

    let name = name.text(db);
    for file in all_matching_files(db, name.as_str()) {
        for range in file.namespace_uses_of(db, package, name.as_str()) {
            results.push(FileRange { file, range });
        }
    }
}

fn find_member_references(db: &dyn Db, file: File, name: &str, kind: MemberKind) -> Vec<FileRange> {
    let mut results = Vec::new();

    for file in all_matching_files(db, name) {
        for range in file.member_uses_of(db, name, kind) {
            results.push(FileRange { file, range });
        }
    }

    sort_file_ranges(&mut results, db, file);
    results
}

fn find_namespace_references(
    db: &dyn Db,
    primary: File,
    namespace: &str,
    name: &str,
) -> Vec<FileRange> {
    let mut results = Vec::new();

    for file in all_matching_files(db, name) {
        for range in file.namespace_uses_of(db, namespace, name) {
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
            r.file.path(db).to_url().to_string(),
            r.range.start(),
        )
    });
    ranges.dedup_by(|a, b| a.file == b.file && a.range == b.range);
}
