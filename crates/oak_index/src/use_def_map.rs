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
// - `record_definition()`: A definition like `x <- 1` kills all previous live
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
// The same primitives handle other control flow: a `while` body may not
// execute, so we snapshot before, visit the body, then merge (like an
// if-without-else). `for` is similar, except the loop variable is always
// bound before the snapshot.
//
// ## Retroactive fixups
//
// The snapshot/restore/merge model is forward-only: a use sees definitions
// recorded before it. Two situations need the opposite: definitions that
// are recorded *after* a use must retroactively reach it. Without this,
// features like rename and jump-to-definition would miss connections.
//
// ### Loop-carried definitions (`finish_loop_defs()`)
//
// ```r
// x <- 0       # def A
// while (cond) {
//     print(x) # should see def A (pre-loop) AND def B (previous iteration)
//     x <- 1   # def B
// }
// ```
//
// When visiting the body top-to-bottom, `print(x)` is recorded before
// `x <- 1`, so it only sees `{A}`. But in a second iteration, `x <- 1`
// from the first iteration should reach `print(x)`. After the body,
// `finish_loop_defs()` diffs the pre-loop and post-body symbol states.
// Any new definition (here, B) is retroactively added to uses of that
// symbol recorded during the body. Result: `print(x)` sees `{A, B}`.
//
// ### Deferred definitions (`record_deferred_definition()`)
//
// `<<-` modifies a symbol that should already be bound in an ancestor scope (if
// there is no existing definition, R stores in the global environment, but
// we'll lint about it). For this reason, `<<-` _adds_ to the set of potential
// definitions reaching uses of that symbols, it doesn't overwrite like `<-`
// would.
//
// ```r
// x <- 0           # def A
// print(x)         # should see def A AND def B
// f <- function() {
//     x <<- 1      # def B (targets file scope)
// }
// ```
//
// Here the `<<-` creates a definition in the file scope, but it's encountered
// during the function body walk, after `print(x)` was already recorded.
// `record_deferred_definition()` adds it to the live state (so future uses
// see it) and also stashes it. At finalization, `finish_deferred_defs()`
// retroactively adds it to all uses of that symbol, including `print(x)`.
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

/// The immutable use-def map for a single scope. For each use site, stores the
/// set of definitions that can reach it through control flow.
#[derive(Debug)]
pub struct UseDefMap {
    bindings_by_use: IndexVec<UseId, Bindings>,
}

impl UseDefMap {
    pub fn bindings_at_use(&self, use_id: UseId) -> &Bindings {
        &self.bindings_by_use[use_id]
    }
}

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
    fn record_definition(&mut self, def_id: DefinitionId) {
        self.definitions.clear();
        self.definitions.push(def_id);
        self.may_be_unbound = false;
    }

    /// Add a definition to the live set without clearing existing ones and
    /// without changing `may_be_unbound`. Used for loop-carried definitions
    /// and scope-wide definitions (`<<-`).
    fn add_definition(&mut self, def_id: DefinitionId) {
        let pos = self.definitions.partition_point(|&id| id < def_id);
        if pos >= self.definitions.len() || self.definitions[pos] != def_id {
            self.definitions.insert(pos, def_id);
        }
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

/// Mutable builder for constructing a [`UseDefMap`] during the tree walk.
/// One builder exists per scope. When entering a nested scope the current
/// builder is pushed onto a stack and a fresh one takes over.
#[derive(Debug)]
pub(crate) struct UseDefMapBuilder {
    symbol_states: IndexVec<SymbolId, Bindings>,
    bindings_by_use: IndexVec<UseId, Bindings>,
    // Maps each use to its symbol, so retroactive fixups (for `<<-` and
    // loop-carried definitions) can find which uses to patch for a given
    // symbol.
    symbol_for_use: IndexVec<UseId, SymbolId>,
    // Definitions whose effect on past uses is deferred to `finish()`.
    // Currently used for `<<-` extra definitions in ancestor scopes.
    deferred_defs: Vec<(SymbolId, DefinitionId)>,
}

impl UseDefMapBuilder {
    pub(crate) fn new() -> Self {
        Self {
            symbol_states: IndexVec::new(),
            bindings_by_use: IndexVec::new(),
            symbol_for_use: IndexVec::new(),
            deferred_defs: Vec::new(),
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
    pub(crate) fn record_definition(&mut self, symbol_id: SymbolId, def_id: DefinitionId) {
        self.symbol_states[symbol_id].record_definition(def_id);
    }

    /// Record a definition whose effect on past uses is deferred to
    /// `finish()`. The definition is added to the current flow state
    /// immediately (so future uses see it), but uses already recorded
    /// are patched up at finalization time. Used for `<<-` extra
    /// definitions.
    pub(crate) fn record_deferred_definition(&mut self, symbol_id: SymbolId, def_id: DefinitionId) {
        self.symbol_states[symbol_id].add_definition(def_id);
        self.deferred_defs.push((symbol_id, def_id));
    }

    /// After visiting a loop body, retroactively patch uses so that
    /// definitions from the bottom of the body reach uses at the top
    /// (simulating a previous iteration).
    ///
    /// Diffs each symbol's definitions before (`pre_loop`) and after the
    /// body. Any definition present after but not before is new (it was
    /// created inside the body). Those new definitions are added to all
    /// uses of that symbol from `first_use` onwards, which covers exactly
    /// the uses recorded during the body.
    ///
    /// This runs after the body (not eagerly at each definition) because
    /// the body may contain branches. A diff at the end captures the
    /// converged state after all snapshot/restore/merge within the body
    /// has resolved.
    pub(crate) fn finish_loop_defs(&mut self, pre_loop: &FlowSnapshot, first_use: UseId) {
        for i in 0..self.symbol_states.len() {
            let symbol_id = SymbolId::new(i);

            let pre_defs = if i < pre_loop.symbol_states.len() {
                pre_loop.symbol_states[symbol_id].definitions()
            } else {
                // Symbol was first interned during the body, so it had
                // no definitions before the loop.
                &[]
            };
            let post_defs = self.symbol_states[symbol_id].definitions();

            // Collect new definitions introduced in the body
            let new_defs: SmallVec<[DefinitionId; 2]> = post_defs
                .iter()
                .filter(|d| !pre_defs.contains(d))
                .copied()
                .collect();

            // Most symbols are unchanged, exit early in that case
            if new_defs.is_empty() {
                continue;
            }

            // Add new defs to uses recorded during the body (`first_use`
            // onwards). Uses before the loop are unaffected.
            for j in first_use.index()..self.bindings_by_use.len() {
                let use_id = UseId::new(j);
                if self.symbol_for_use[use_id] == symbol_id {
                    for &def_id in &new_defs {
                        self.bindings_by_use[use_id].add_definition(def_id);
                    }
                }
            }
        }
    }

    /// Record a use of `symbol_id`. Clones the current live bindings for that
    /// symbol and associates them with `use_id`.
    pub(crate) fn record_use(&mut self, symbol_id: SymbolId, use_id: UseId) {
        let bindings = self.symbol_states[symbol_id].clone();
        let pushed_id = self.bindings_by_use.push(bindings);
        self.symbol_for_use.push(symbol_id);
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
    pub(crate) fn finish(mut self) -> UseDefMap {
        self.finish_deferred_defs();
        UseDefMap {
            bindings_by_use: self.bindings_by_use,
        }
    }

    /// Retroactively add deferred definitions (from `<<-`) to all
    /// uses of the corresponding symbol, including uses that were
    /// recorded before the definition was encountered in the walk.
    fn finish_deferred_defs(&mut self) {
        for &(symbol_id, def_id) in &self.deferred_defs {
            for i in 0..self.bindings_by_use.len() {
                let use_id = UseId::new(i);
                if self.symbol_for_use[use_id] == symbol_id {
                    self.bindings_by_use[use_id].add_definition(def_id);
                }
            }
        }
    }
}

/// A snapshot of all symbol states at a particular point in control flow.
#[derive(Clone, Debug)]
pub(crate) struct FlowSnapshot {
    symbol_states: IndexVec<SymbolId, Bindings>,
}
