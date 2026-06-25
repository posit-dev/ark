use std::cell::Cell;
use std::rc::Rc;

use oak_semantic::effects_registry;
use oak_semantic::Effects;
use oak_semantic::ImportsResolver;
use oak_semantic::SourceResolution;

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
}

impl TestImportsResolver {
    /// Resolver with base always attached: the minimum for the bare base NSE
    /// functions (`local`, `with`, `within`, `evalq`) to resolve.
    pub fn with_base() -> Self {
        Self {
            always_attached: vec![String::from("base")],
            consultations: Rc::new(Cell::new(0)),
        }
    }

    /// A handle to the consultation counter. Clone it before moving the
    /// resolver into `build_index`, then read it after the build.
    pub fn consultations(&self) -> Rc<Cell<usize>> {
        Rc::clone(&self.consultations)
    }
}

impl ImportsResolver for TestImportsResolver {
    fn resolve_source(&mut self, _path: &str) -> Option<SourceResolution> {
        None
    }

    fn resolve_effects(&mut self, name: &str, attached: &[String], _lazy: bool) -> Option<Effects> {
        self.consultations.set(self.consultations.get() + 1);
        attached
            .iter()
            .rev()
            .chain(self.always_attached.iter())
            .find_map(|pkg| effects_registry::lookup(pkg, name).copied())
            .map(Effects::nse)
    }
}
