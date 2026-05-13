//! Bridge between editor-facing URLs and `oak_db`'s salsa inputs.
//!
//! [`Vfs`] caches URL canonicalisation and indexes `Root` and `Package`
//! entities by canonical URL. File-by-URL lookup goes through the
//! [`Files`](crate::Files) interner (which lives on the concrete `db`),
//! so the Vfs doesn't carry its own file index.
//!
//! `update_file`, `remove_file`, and `rename_file` are the editor-event
//! surface. The API names operations rather than events, so a
//! notify-based watcher could drive the same surface.

use std::collections::HashMap;

use aether_url::UrlId;
use salsa::Setter;
use url::Url;

use crate::intern_file;
use crate::vfs_scan::FileDescriptor;
use crate::vfs_scan::PackageDescriptor;
use crate::vfs_scan::ScanResult;
use crate::Db;
use crate::File;
use crate::Package;
use crate::PackageOrigin;
use crate::Root;
use crate::RootKind;
use crate::Script;
use crate::SourceNode;

#[derive(Default)]
pub struct Vfs {
    /// Cache of `UrlId::from_canonical` results so LSP event handlers
    /// don't re-canonicalise on every call. The key is the raw editor
    /// URL, the value is the salsa-side canonical form.
    url_id_by_url: HashMap<Url, UrlId>,
    /// Index from package root URL to `Package`. Used by `update_file`
    /// to set the back-pointer on a newly-interned file and by
    /// `apply_scan` to upsert packages.
    package_by_root: HashMap<UrlId, Package>,
    /// Index from workspace root URL to `Root`. Used to bump
    /// `Root.revision` after file-set changes inside that root.
    root_by_url: HashMap<UrlId, Root>,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve a raw editor URL to the salsa `File` entity, if any.
    pub fn url_to_file<DB: Db>(&self, db: &DB, url: &Url) -> Option<File> {
        let canonical = self.url_id_by_url.get(url)?;
        db.files().get(canonical)
    }

    /// Create or update a single file's contents.
    ///
    /// Routes through [`intern_file`] so the file is registered in the
    /// `Files` interner and (for new files under a known package root)
    /// gets its `parent` back-pointer set. Bumps the containing
    /// `Root.revision` on first intern so `collation_files` /
    /// `script_by_url` re-execute. Subsequent content-only edits
    /// don't bump.
    pub fn update_file<DB: Db>(&mut self, db: &mut DB, url: Url, contents: String) -> File {
        let url_id = self.canonicalize(url);

        if let Some(existing) = db.files().get(&url_id) {
            existing.set_contents(db).to(contents);
            return existing;
        }

        let parent = self.parent_for(&url_id);
        let file = intern_file(db, url_id.clone(), contents, parent);

        if let Some(SourceNode::Script(_)) = parent {
            let source_graph = db.source_graph();
            let mut scripts = source_graph.scripts(db).clone();
            scripts.push(Script::new(db, file));
            source_graph.set_scripts(db).to(scripts);
        } else if parent.is_none() {
            // Orphan file under no package root. Wrap in a `Script` and
            // attach it to the source graph so jump-to-def can reach it.
            let script = Script::new(db, file);
            file.set_parent(db).to(Some(SourceNode::Script(script)));
            let source_graph = db.source_graph();
            let mut scripts = source_graph.scripts(db).clone();
            scripts.push(script);
            source_graph.set_scripts(db).to(scripts);
        }

        self.bump_root_for(db, &url_id);
        file
    }

    /// Remove a file. Drops the URL mapping, removes the file from the
    /// `Files` interner, detaches it from `SourceGraph.scripts` if it
    /// was an orphan, and bumps the containing `Root.revision` so
    /// `collation_files` and `script_by_url` re-execute. The
    /// `Package.collation` spec stays untouched. The basename simply
    /// drops from `collation_files`'s next snapshot.
    pub fn remove_file<DB: Db>(&mut self, db: &mut DB, url: &Url) {
        let Some(url_id) = self.url_id_by_url.remove(url) else {
            return;
        };
        let Some(file) = db.files().remove(&url_id) else {
            return;
        };

        if let Some(SourceNode::Script(script)) = file.parent(db) {
            let source_graph = db.source_graph();
            let scripts: Vec<Script> = source_graph
                .scripts(db)
                .iter()
                .copied()
                .filter(|s| *s != script)
                .collect();
            source_graph.set_scripts(db).to(scripts);
        }

        self.bump_root_for(db, &url_id);
    }

    /// Rename a file. Preserves contents, drops the old URL mapping.
    pub fn rename_file<DB: Db>(&mut self, db: &mut DB, old: &Url, new: Url) {
        let Some(canonical) = self.url_id_by_url.get(old).cloned() else {
            return;
        };
        let Some(file) = db.files().get(&canonical) else {
            return;
        };
        let contents = file.contents(db).clone();
        self.remove_file(db, old);
        self.update_file(db, new, contents);
    }

    /// Replace the workspace folders. Canonicalises URLs, allocates
    /// fresh `Root` entities for new folders, and writes the resulting
    /// list to `WorkspaceRoots`.
    pub fn set_workspace_roots<DB: Db>(&mut self, db: &mut DB, urls: Vec<Url>) {
        let mut roots = Vec::with_capacity(urls.len());
        for url in urls {
            let canonical = self.canonicalize(url);
            let root = *self
                .root_by_url
                .entry(canonical.clone())
                .or_insert_with(|| Root::new(db, canonical.clone(), RootKind::Workspace, 0));
            roots.push(root);
        }
        db.workspace_roots().set_roots(db).to(roots);
    }

    /// Apply a [`ScanResult`] in bulk. Upserts packages and standalone
    /// scripts, preserving salsa entities for paths already known to
    /// the Vfs so downstream queries keep their cached results when
    /// possible.
    pub fn apply_scan<DB: Db>(&mut self, db: &mut DB, scan: ScanResult) {
        let mut workspace_packages = Vec::new();
        for desc in scan.packages {
            workspace_packages.push(self.apply_package(db, desc));
        }
        db.source_graph()
            .set_workspace_packages(db)
            .to(workspace_packages);

        let mut scripts = Vec::new();
        for file_desc in scan.scripts {
            if let Some(script) = self.upsert_script(db, &file_desc) {
                scripts.push(script);
            }
        }
        db.source_graph().set_scripts(db).to(scripts);
    }

    fn apply_package<DB: Db>(&mut self, db: &mut DB, desc: PackageDescriptor) -> Package {
        let Ok(root_url) = Url::from_directory_path(&desc.root) else {
            log::warn!("Can't convert package root to URL: {}", desc.root.display());
            return self.fallback_package(db, desc);
        };
        let root_id = self.canonicalize(root_url);
        let root = *self
            .root_by_url
            .entry(root_id.clone())
            .or_insert_with(|| Root::new(db, root_id.clone(), RootKind::Workspace, 0));

        let package = if let Some(&existing) = self.package_by_root.get(&root_id) {
            existing.set_name(db).to(desc.name.clone());
            existing.set_namespace(db).to(desc.namespace.clone());
            existing.set_collation(db).to(desc.collation_spec.clone());
            existing
        } else {
            Package::new(
                db,
                desc.name.clone(),
                PackageOrigin::Workspace { root },
                desc.namespace.clone(),
                desc.collation_spec.clone(),
            )
        };

        for file_desc in &desc.files {
            self.upsert_file_at_path(db, file_desc, Some(SourceNode::Package(package)));
        }

        // Bump after interning so `collation_files` re-derives.
        let next = root.revision(db) + 1;
        root.set_revision(db).to(next);

        self.package_by_root.insert(root_id, package);
        package
    }

    fn upsert_script<DB: Db>(&mut self, db: &mut DB, desc: &FileDescriptor) -> Option<Script> {
        let url = Url::from_file_path(&desc.path).ok()?;
        let canonical = self.canonicalize(url);

        if let Some(existing) = db.files().get(&canonical) {
            existing.set_contents(db).to(desc.contents.clone());
            if let Some(SourceNode::Script(s)) = existing.parent(db) {
                return Some(s);
            }
            let script = Script::new(db, existing);
            existing.set_parent(db).to(Some(SourceNode::Script(script)));
            return Some(script);
        }

        let file = intern_file(db, canonical, desc.contents.clone(), None);
        let script = Script::new(db, file);
        file.set_parent(db).to(Some(SourceNode::Script(script)));
        Some(script)
    }

    fn upsert_file_at_path<DB: Db>(
        &mut self,
        db: &mut DB,
        desc: &FileDescriptor,
        parent: Option<SourceNode>,
    ) -> Option<File> {
        let url = Url::from_file_path(&desc.path).ok()?;
        let canonical = self.canonicalize(url);
        Some(intern_file(db, canonical, desc.contents.clone(), parent))
    }

    fn canonicalize(&mut self, url: Url) -> UrlId {
        if let Some(id) = self.url_id_by_url.get(&url) {
            return id.clone();
        }
        let id = UrlId::from_canonical(url.clone());
        self.url_id_by_url.insert(url, id.clone());
        id
    }

    /// `parent` value for a newly-interned file at `url_id`. Returns
    /// `Some(SourceNode::Package(p))` if `url_id` is under a known
    /// package's `R/`. Returns `None` otherwise; the caller (e.g.
    /// `update_file`) is responsible for setting up a `Script`
    /// wrapper when appropriate.
    fn parent_for(&self, url_id: &UrlId) -> Option<SourceNode> {
        let file_path = url_id.to_file_path()?;
        self.package_by_root
            .iter()
            .filter_map(|(root_id, package)| {
                let root_path = root_id.to_file_path()?;
                let r_dir = root_path.join("R");
                if file_path.starts_with(&r_dir) {
                    Some((root_path, *package))
                } else {
                    None
                }
            })
            .max_by_key(|(root, _)| root.components().count())
            .map(|(_, pkg)| SourceNode::Package(pkg))
    }

    /// Bump the `Root.revision` for the workspace root containing
    /// `url_id`, if any. Hook for `update_file` / `remove_file` to
    /// trip the salsa anchor on `collation_files` and
    /// `script_by_url`.
    fn bump_root_for<DB: Db>(&self, db: &mut DB, url_id: &UrlId) {
        let Some(file_path) = url_id.to_file_path() else {
            return;
        };
        let Some(root) = self
            .root_by_url
            .iter()
            .filter_map(|(root_url, root)| {
                let root_path = root_url.to_file_path()?;
                if file_path.starts_with(&root_path) {
                    Some((root_path, *root))
                } else {
                    None
                }
            })
            .max_by_key(|(path, _)| path.components().count())
            .map(|(_, root)| root)
        else {
            return;
        };
        let next = root.revision(db) + 1;
        root.set_revision(db).to(next);
    }

    /// Package fallback for the rare case where the package root path
    /// can't be turned into a URL. Allocates a placeholder `Root`
    /// (file:///) and registers the package against it. The package
    /// works for namespace / NAMESPACE lookups but `collation_files`
    /// returns empty because no files share the placeholder root.
    fn fallback_package<DB: Db>(&mut self, db: &mut DB, desc: PackageDescriptor) -> Package {
        let placeholder = UrlId::from_canonical(Url::parse("file:///").expect("static URL parses"));
        let root = *self
            .root_by_url
            .entry(placeholder.clone())
            .or_insert_with(|| Root::new(db, placeholder, RootKind::Workspace, 0));
        Package::new(
            db,
            desc.name,
            PackageOrigin::Workspace { root },
            desc.namespace,
            desc.collation_spec,
        )
    }
}
