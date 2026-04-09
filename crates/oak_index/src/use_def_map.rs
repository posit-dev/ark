use itertools::EitherOrBoth;
use itertools::Itertools;
use smallvec::SmallVec;

use crate::index_vec::Idx;
use crate::index_vec::IndexVec;
use crate::semantic_index::DefinitionId;
use crate::semantic_index::SymbolId;
use crate::semantic_index::UseId;

// Use-def tracking answers: "at this use of `x`, which specific definitions
// could have run?" In straight-line code it's trivial: each definition shadows
// the previous one, and a use sees whatever definition came last. The
// complexity comes from branching.
//
// For each symbol in the current scope, we track a `Bindings`: the set of
// `DefinitionId`s that are "live". A fresh scope starts with every symbol in
// the "unbound" state: empty definition set, `may_be_unbound: true`.
// The "may_be_unbound" flag tracks whether there exists some control flow path
// where no definition was reached.
//
// ```r
// if (cond) {
//     x <- 1  # def A
// }
// print(x) # may_be_unbound: true, definitions: {A}
// ```
//
// - `record_binding()`: A definition like `x <- 1` kills all previous live
//   definitions for that symbol and replaces them with a singleton.
//   `may_be_unbound` becomes false.
//
// - `record_use()`: A use like `print(x)` freezes the current live state for
//   that symbol. We clone the current `Bindings` and store it keyed by `use_id`.
//   This operation doesn't change any state.
//
// The other operations (`snapshot()`, `restore()`, `merge()`) help dealing with
// control flow complications.
//
// ```r
// x <- 1     # def A
// if (cond) {
//     x <- 2 # def B
// } else {
//     x <- 3 # def C
// }
// print(x)   # which defs reach this use?
// ```
//
// 1. `snapshot()`: Clone the entire symbol state (all symbols' live
//    definitions). This captures the state *before* either branch runs. Call
//    this `pre_if`.
// 2. Visit the if-body: `x <- 2` shadows, so `x`'s state becomes `{B}`.
// 3. `snapshot()` again: capture the post-if-body state. Call this `post_if`.
// 4. `restore(pre_if)`: Reset to the state before the if-body ran. Now `x`'s
//    state is back to `{A}`.
// 5. Visit the else-body. `x <- 3` shadows, so `x`'s state becomes `{C}`.
// 6. `merge(post_if)`: For each symbol, union the current state (post-else:
//    `{C}`) with the snapshot (post-if: `{B}`). Result: `x` has `{B, C}`.
//
// After this, `print(x)` records a use that sees `{B, C}`. Def A is gone
// because both branches shadowed it.
//
// If there's no else clause, step 5 is skipped. The current state after
// restore is still `pre_if` (`{A}`). Merge unions `{A}` with `{B}` → `{A, B}`.
// The pre-if definition stays live because there's a path (the no-else path)
// where it wasn't shadowed.
//
// The same primitives can be used to implement other control flow constructs
// following similar considerations (the body of a loop might not execute).
//
// ## Interpreting `Bindings`
//
// Callers examine the two fields of `Bindings` at a use site to determine
// what happened along control flow:
//
//   definitions | may_be_unbound | meaning
//   ------------|----------------|--------
//   {A}         | false          | one definition, straight-line
//   {B, C}      | false          | paths converge (e.g. both if/else branches)
//   {A, B}      | false          | prior def + conditional redefinition
//   {A}         | true           | conditional definition (e.g. if without else)
//   {}          | true           | no local def, parent scope reference
//   {A, B}      | true           | some paths define, some don't

/// The set of definitions that can reach a particular point in control flow,
/// plus whether the symbol may be unbound (no definition on some path).
///
/// Definitions are stored sorted by ID so that merge is a linear merge-join. The
/// `SmallVec<[DefinitionId; 2]>` avoids heap allocation for the common case of
/// 1-2 live definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    definitions: SmallVec<[DefinitionId; 2]>,
    may_be_unbound: bool,
}

impl Bindings {
    fn unbound() -> Self {
        Self {
            definitions: SmallVec::new(),
            may_be_unbound: true,
        }
    }

    pub fn definitions(&self) -> &[DefinitionId] {
        &self.definitions
    }

    pub fn contains_definition(&self, id: DefinitionId) -> bool {
        self.definitions.contains(&id)
    }

    pub fn definition_count(&self) -> usize {
        self.definitions.len()
    }

    pub fn may_be_unbound(&self) -> bool {
        self.may_be_unbound
    }

    /// Replace all live definitions with a single new one, marking the
    /// symbol as definitely bound.
    fn record_binding(&mut self, def_id: DefinitionId) {
        self.definitions.clear();
        self.definitions.push(def_id);
        self.may_be_unbound = false;
    }

    /// Union definitions from `other` into `self`, OR the `may_be_unbound`
    /// flags. Both sides are sorted by `DefinitionId`, so this is a linear
    /// merge-join.
    fn merge(&mut self, other: Bindings) {
        self.definitions = sorted_union(&self.definitions, &other.definitions);
        self.may_be_unbound |= other.may_be_unbound;
    }
}

/// Merge two sorted slices into a sorted `SmallVec` with no duplicates.
fn sorted_union(a: &[DefinitionId], b: &[DefinitionId]) -> SmallVec<[DefinitionId; 2]> {
    a.iter()
        .merge_join_by(b.iter(), |a, b| a.cmp(b))
        .map(|either| match either {
            EitherOrBoth::Left(&id) | EitherOrBoth::Right(&id) | EitherOrBoth::Both(&id, _) => id,
        })
        .collect()
}

/// A snapshot of all symbol states at a particular point in control flow.
#[derive(Clone, Debug)]
pub(crate) struct FlowSnapshot {
    symbol_states: IndexVec<SymbolId, Bindings>,
}

/// The immutable use-def map for a single scope, produced by finalizing the
/// builder. For each use site, stores the set of definitions that can reach
/// it through control flow.
#[derive(Debug)]
pub struct UseDefMap {
    bindings_by_use: IndexVec<UseId, Bindings>,
}

impl UseDefMap {
    pub(crate) fn empty() -> Self {
        Self {
            bindings_by_use: IndexVec::new(),
        }
    }

    pub fn bindings_at_use(&self, use_id: UseId) -> &Bindings {
        &self.bindings_by_use[use_id]
    }
}

/// Mutable builder for constructing a [`UseDefMap`] during the tree walk.
/// One builder exists per scope. When entering a nested scope the current
/// builder is pushed onto a stack and a fresh one takes over.
#[derive(Debug)]
pub(crate) struct UseDefMapBuilder {
    symbol_states: IndexVec<SymbolId, Bindings>,
    bindings_by_use: IndexVec<UseId, Bindings>,
}

impl UseDefMapBuilder {
    pub(crate) fn new() -> Self {
        Self {
            symbol_states: IndexVec::new(),
            bindings_by_use: IndexVec::new(),
        }
    }

    /// Ensure that `symbol_id` has an entry in `symbol_states`, growing the
    /// vec with "unbound" entries as needed. Called after interning a symbol
    /// so the use-def state stays in sync with the symbol table.
    pub(crate) fn ensure_symbol(&mut self, symbol_id: SymbolId) {
        // In practice this adds at most one entry (IDs are sequential), the
        // `while` is defensive.
        while self.symbol_states.len() <= symbol_id.index() {
            self.symbol_states.push(Bindings::unbound());
        }
    }

    /// Record a new binding for `symbol_id`. Replaces (shadows) all previous
    /// live definitions for that symbol.
    pub(crate) fn record_binding(&mut self, symbol_id: SymbolId, def_id: DefinitionId) {
        self.symbol_states[symbol_id].record_binding(def_id);
    }

    /// Record a use of `symbol_id`. Clones the current live bindings for that
    /// symbol and associates them with `use_id`.
    pub(crate) fn record_use(&mut self, symbol_id: SymbolId, use_id: UseId) {
        let bindings = self.symbol_states[symbol_id].clone();
        let pushed_id = self.bindings_by_use.push(bindings);
        stdext::soft_assert!(use_id == pushed_id);
    }

    /// Take a snapshot of the current symbol states.
    pub(crate) fn snapshot(&self) -> FlowSnapshot {
        FlowSnapshot {
            symbol_states: self.symbol_states.clone(),
        }
    }

    /// Restore state to a previously taken snapshot.
    pub(crate) fn restore(&mut self, snapshot: FlowSnapshot) {
        let num_symbols = self.symbol_states.len();
        self.symbol_states = snapshot.symbol_states;

        // New symbols may have been interned between snapshot and restore.
        // Fill them in as "unbound" so IDs stay aligned.
        while self.symbol_states.len() < num_symbols {
            self.symbol_states.push(Bindings::unbound());
        }
    }

    /// Merge a snapshot into the current state. For each symbol, union the
    /// definition sets and OR the `may_be_unbound` flags. This reflects that
    /// control flow could have taken either path to reach this point.
    pub(crate) fn merge(&mut self, snapshot: FlowSnapshot) {
        let mut snap_iter = snapshot.symbol_states.into_iter();

        for i in 0..self.symbol_states.len() {
            let id = SymbolId::new(i);
            if let Some(snap_bindings) = snap_iter.next() {
                self.symbol_states[id].merge(snap_bindings);
            } else {
                // Symbol didn't exist in the snapshot, so it was unbound on
                // that path
                self.symbol_states[id].merge(Bindings::unbound());
            }
        }
    }

    /// Finalize into an immutable [`UseDefMap`].
    pub(crate) fn finish(self) -> UseDefMap {
        UseDefMap {
            bindings_by_use: self.bindings_by_use,
        }
    }
}
