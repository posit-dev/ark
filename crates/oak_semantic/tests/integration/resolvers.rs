use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oak_semantic::effects;
use oak_semantic::EffectsHandlers;
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
    /// Per-consultation `(name, lazy)` records, so tests can pin the `lazy` flag
    /// the builder derives from the callee's context.
    consultation_log: Rc<RefCell<Vec<(String, bool)>>>,
    /// `source()` paths this resolver knows, mapped to the names they export.
    sources: HashMap<String, SourceResolution>,
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
            consultation_log: Rc::new(RefCell::new(Vec::new())),
            sources: HashMap::new(),
        }
    }

    /// Register a sourced file at `path` exporting `names`, so `resolve_source`
    /// returns a resolution for it. The URL is synthesized from the path.
    pub fn with_source(mut self, path: &str, names: &[&str]) -> Self {
        let resolution = SourceResolution {
            url: Url::parse(&format!("file:///{path}")).unwrap(),
            names: names.iter().map(|name| name.to_string()).collect(),
            packages: vec![],
        };
        self.sources.insert(path.to_string(), resolution);
        self
    }

    /// A handle to the consultation counter. Clone it before moving the
    /// resolver into `build_index`, then read it after the build.
    pub fn consultations(&self) -> Rc<Cell<usize>> {
        Rc::clone(&self.consultations)
    }

    /// A handle to the per-consultation `(name, lazy)` log. Clone it before
    /// moving the resolver into `build_index`, then read it after the build.
    pub fn consultation_log(&self) -> Rc<RefCell<Vec<(String, bool)>>> {
        Rc::clone(&self.consultation_log)
    }
}

impl ImportsResolver for TestImportsResolver {
    fn resolve_source(&mut self, path: &str) -> Option<SourceResolution> {
        self.sources.get(path).cloned()
    }

    fn resolve_effects(
        &mut self,
        name: &str,
        attached: &[String],
        lazy: bool,
    ) -> Option<EffectsHandlers> {
        self.consultations.set(self.consultations.get() + 1);
        self.consultation_log
            .borrow_mut()
            .push((name.to_string(), lazy));
        attached
            .iter()
            .rev()
            .chain(self.always_attached.iter())
            .find_map(|pkg| effects::lookup(pkg, name).copied())
    }
}
