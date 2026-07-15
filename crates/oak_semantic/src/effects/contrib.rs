use std::sync::LazyLock;

use crate::effects::Declaration;
use crate::effects::EffectSource;
use crate::effects::Handler;

mod base;
mod magrittr;
mod rlang;
mod s7;
mod shiny;
mod testthat;

/// One registry entry: a `(package, function)` pair and the effect source it
/// contributes. Owns its payload, an inline [`Declaration`] (data) or a
/// `&'static dyn Handler` (code), matching the [`EffectSource`] split.
pub(crate) struct Entry {
    pub(super) package: &'static str,
    pub(super) function: &'static str,
    payload: EntryPayload,
}

/// A registry entry is declarative xor custom, never per-axis mixable: in
/// practice a function is all-declarative (`local`, `library`) or all-custom
/// (bquote, binding operators, assign), never one axis of each.
enum EntryPayload {
    Declared(Declaration),
    Custom(&'static dyn Handler),
}

impl Entry {
    /// Project an entry into the `Copy` [`EffectSource`]. The `&'static`
    /// borrows come from the `LazyLock`-owned registry.
    pub(super) fn source(&'static self) -> EffectSource {
        match &self.payload {
            EntryPayload::Declared(declaration) => EffectSource::Declared(declaration),
            EntryPayload::Custom(handler) => EffectSource::Custom(*handler),
        }
    }
}

/// Build a declarative entry from an owned [`Declaration`].
pub(super) fn declared(
    package: &'static str,
    function: &'static str,
    declaration: Declaration,
) -> Entry {
    Entry {
        package,
        function,
        payload: EntryPayload::Declared(declaration),
    }
}

/// Build a custom entry from a `&'static dyn Handler`.
pub(super) fn custom(
    package: &'static str,
    function: &'static str,
    handler: &'static dyn Handler,
) -> Entry {
    Entry {
        package,
        function,
        payload: EntryPayload::Custom(handler),
    }
}

/// The effect registry, assembled once from each package's contributions.
///
/// A `LazyLock<Vec<Entry>>` rather than a `static` table because an owned
/// [`Declaration`] holds `Vec`s and can't sit in a `static`.
pub(super) static REGISTRY: LazyLock<Vec<Entry>> = LazyLock::new(|| {
    [
        base::entries(),
        magrittr::entries(),
        rlang::entries(),
        s7::entries(),
        shiny::entries(),
        testthat::entries(),
    ]
    .into_iter()
    .flatten()
    .collect()
});
