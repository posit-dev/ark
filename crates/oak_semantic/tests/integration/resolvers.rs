use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use oak_semantic::effects;
use oak_semantic::effects::DirWalk;
use oak_semantic::FunctionHandlers;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;
use url::Url;

/// Test resolver: an explicit search path resolved against the registry.
///
/// Resolves a bare callee by walking `attached` (LIFO) then its own
/// always-attached packages, returning the first package's registry annotation
/// for the name. Base is a normal entry in the always-attached list, not a
/// special case. Flat: no re-export chase, that's the salsa resolver's job.
pub struct TestImportsResolver {
    /// Packages always on the search path, base last. These stand in for the
    /// non-flow layers (base, default search path) the salsa resolver derives.
    always_attached: Vec<String>,
    /// Count of `resolve_effects` consultations, so tests can assert the front
    /// gate keeps unannotated names off the resolver.
    consultations: Rc<Cell<usize>>,
    /// `source()` paths this resolver knows, mapped to the names they export.
    sources: HashMap<String, SourceResolution>,
    /// Directory listings keyed by path and walk mode, so a handler that asks
    /// for the wrong `DirWalk` gets no files rather than a silent match.
    source_dirs: HashMap<(String, DirWalk), Vec<SourceResolution>>,
    /// Explicit exports keep locked-binding checks independent of the effects
    /// registry, which describes call semantics rather than binding existence.
    exports: HashMap<String, Vec<String>>,
}

impl TestImportsResolver {
    /// Resolver with base always attached: the minimum for the bare base NSE
    /// functions (`local`, `with`, `within`, `evalq`) to resolve.
    pub fn with_base() -> Self {
        Self::with_attached(&[])
    }

    /// Resolver with `packages` always attached, plus base last. For effects
    /// contributed by a package that would otherwise need a `library()` call to
    /// enter the flow-precise attach set, e.g. magrittr's `%<>%` operator.
    pub fn with_attached(packages: &[&str]) -> Self {
        let mut always_attached: Vec<String> = packages.iter().map(|pkg| pkg.to_string()).collect();
        always_attached.push(String::from("base"));
        Self {
            always_attached,
            consultations: Rc::new(Cell::new(0)),
            sources: HashMap::new(),
            source_dirs: HashMap::new(),
            exports: HashMap::new(),
        }
    }

    /// Declare that `package` exports `names`, making them locked `<<-`
    /// targets when `package` is on the search path.
    pub fn with_exports(mut self, package: &str, names: &[&str]) -> Self {
        self.exports.insert(
            package.to_string(),
            names.iter().map(|name| name.to_string()).collect(),
        );
        self
    }

    /// Register a sourced file at `path` exporting `names`, so `resolve_source`
    /// returns a resolution for it. The URL is synthesized from the path.
    pub fn with_source(mut self, path: &str, names: &[&str]) -> Self {
        self.sources
            .insert(path.to_string(), source_resolution(path, names));
        self
    }

    /// Register a directory at `path` listed with `walk`, so
    /// `resolve_source_dir` returns one resolution per `(file_path,
    /// exported_names)` entry, in order. URLs are synthesized from the paths.
    pub fn with_source_dir(mut self, path: &str, walk: DirWalk, files: &[(&str, &[&str])]) -> Self {
        let resolutions = files
            .iter()
            .map(|(file, names)| source_resolution(file, names))
            .collect();
        self.source_dirs
            .insert((path.to_string(), walk), resolutions);
        self
    }

    /// A handle to the consultation counter. Clone it before moving the
    /// resolver into `build_index`, then read it after the build.
    pub fn consultations(&self) -> Rc<Cell<usize>> {
        Rc::clone(&self.consultations)
    }
}

impl ImportsResolver for TestImportsResolver {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        self.sources.get(path).cloned()
    }

    fn resolve_source_dir(&mut self, path: &str, walk: DirWalk) -> Vec<SourceResolution> {
        self.source_dirs
            .get(&(path.to_string(), walk))
            .cloned()
            .unwrap_or_default()
    }

    fn resolve_effects(&mut self, name: &str, attached: &[String]) -> Option<FunctionHandlers> {
        self.consultations.set(self.consultations.get() + 1);
        attached
            .iter()
            .rev()
            .chain(self.always_attached.iter())
            .find_map(|pkg| effects::lookup(pkg, name).copied())
    }

    fn binding_package(&mut self, name: &str, attached: &[String]) -> Option<String> {
        attached
            .iter()
            .rev()
            .chain(self.always_attached.iter())
            .find(|pkg| {
                self.exports
                    .get(*pkg)
                    .is_some_and(|names| names.iter().any(|export| export == name))
            })
            .cloned()
    }
}

fn source_resolution(path: &str, names: &[&str]) -> SourceResolution {
    SourceResolution {
        url: Url::parse(&format!("file:///{path}")).unwrap(),
        names: names.iter().map(|name| name.to_string()).collect(),
        packages: vec![],
    }
}

/// Resolves base effects (so `library()` itself is recognized as an attach
/// call) but reports every package as not installed. For asserting the
/// builder's `UninstalledPackage` diagnostic at an attach site.
pub struct MissingPackageResolver;

impl ImportsResolver for MissingPackageResolver {
    fn resolve_source(&mut self, _path: &str) -> Option<SourceResolution> {
        None
    }

    fn resolve_effects(&mut self, name: &str, _: &[String]) -> Option<FunctionHandlers> {
        effects::lookup("base", name).copied()
    }

    fn package_exists(&mut self, _package: &str) -> bool {
        false
    }
}
