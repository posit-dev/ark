use aether_syntax::AnyRExpression;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use aether_syntax::RSyntaxKind;
use biome_rowan::AstNode;
use biome_rowan::TextRange;
use oak_core::syntax_ext::AnyRSelectorExt;
use oak_core::syntax_ext::RIdentifierExt;

use super::scan::ScanBindings;
use super::SemanticIndexBuilder;
use crate::effects;
use crate::effects::AssignBinding;
use crate::effects::CallContext;
use crate::effects::EffectSite;
use crate::effects::Effects;
use crate::effects::EffectsHandlers;
use crate::resolver::ImportsResolver;
use crate::semantic_index::SemanticDiagnostic;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    pub(super) fn resolve_effects(&mut self, call: &RCall) -> Option<Effects> {
        let handlers = self.resolve_effects_handlers(call)?;

        // `resolve_effects_handlers()` returns owned handlers, so its `&mut
        // self` borrow is finished. Reborrow immutably.
        let bindings = ScanBindings { builder: &*self };
        let ctx = CallContext::with_bindings(&bindings);

        let arguments = handlers
            .arguments
            .and_then(|handler| handler.resolve(call, &ctx));
        let attach = handlers
            .attach
            .and_then(|handler| handler.resolve(call, &ctx));
        let source = handlers
            .source
            .and_then(|handler| handler.resolve(call, &ctx));
        let assign = handlers
            .assign
            .and_then(|handler| handler.resolve(EffectSite::Call(call), &ctx));

        Some(Effects {
            arguments,
            attach,
            source,
            assign,
        })
    }

    /// Resolve a call's callee to its [`EffectsHandlers`] (NSE, attach, ...).
    ///
    /// The shared core for both NSE recognition ([`scan_call`] reads `.arguments`) and
    /// attach recognition ([`scan_call`] reads `.attach`). Two cases resolve:
    /// - A bare identifier. If bound locally it goes through the local
    ///   [`resolve_local_effects`](Self::resolve_local_effects). Otherwise the
    ///   cross-file `ImportsResolver::resolve_effects()` resolves it across the
    ///   search path, against the attach set in `attached_flow`.
    /// - A `pkg::fn` namespace expression, resolved through
    ///   `ImportsResolver::resolve_qualified_effects()`. `::` names the package,
    ///   so there's no search-path disambiguation; the resolver answers from
    ///   per-package knowledge (the static registry, plus cross-file knowledge
    ///   like the re-export chase once that lands).
    ///
    /// The bound check reads the scan pass's flow-precise binding state
    /// for the current scope, so this must run during the scan, not the walk.
    ///
    /// [`EffectsHandlers`]: crate::effects::EffectsHandlers
    /// [`scan_call`]: Self::scan_call
    fn resolve_effects_handlers(&mut self, call: &RCall) -> Option<EffectsHandlers> {
        let func = call.function().ok()?;

        match &func {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();
                self.resolve_symbol_effects(&name, call.syntax().text_trimmed_range())
            },

            AnyRExpression::RNamespaceExpression(ns_expr) => {
                let left = ns_expr.left().ok()?;
                let right = ns_expr.right().ok()?;
                let pkg = left.identifier_text()?;
                let func_name = right.identifier_text()?;

                if !effects::annotates(&func_name) {
                    return None;
                }

                self.resolver.resolve_qualified_effects(&pkg, &func_name)
            },

            _ => None,
        }
    }

    /// Resolve a callee `sym` to its [`EffectsHandlers`].
    ///
    /// `range` is the invocation's range, used to anchor a lazy-shadow
    /// diagnostic.
    fn resolve_symbol_effects(&mut self, sym: &str, range: TextRange) -> Option<EffectsHandlers> {
        // First check for a local definition (which in the future may
        // carry declared effects that we resolve here)
        //
        // Looked up from `flow_state` which already carries every
        // eager binding visible here: the scope's own flow-precise
        // bindings so far, plus the enclosing eager environment seeded
        // at `begin_scan()`. Forward and deferred (lazy-routed)
        // bindings are excluded. A forward one isn't in `flow_state`
        // yet, and a deferred one (`on_load`, `<<-`) never enters it.
        if self.scan.flow_state.is_bound(sym) {
            return self.resolve_local_effects(sym);
        }

        // Bail early if it is known that no package annotates this name
        // with effects. This speeds up the common case of no known annotations.
        if !effects::annotates(sym) {
            return None;
        }

        // Now check imports since the symbol is locally unbound. The
        // arena's `current_scope` is the scan unit's scope (the descent
        // pushes no arena scopes), so its laziness is the "am I in a lazy
        // context" test the resolver needs. `attached_flow` is the
        // flow-precise attach prefix during the file scan and the
        // complete end-of-file set during the walk.
        let lazy = self.scopes[self.current_scope].kind.is_lazy();
        let effects = self
            .resolver
            .resolve_effects(sym, &self.scan.attached_flow, lazy)?;

        // The callee is unbound by any eager binding, so its effect
        // holds. If a lazy-crossed ancestor binds it whole-scope, that
        // binding's timing relative to this deferred body is
        // undetermined, so the decision is a guess. Flag it.
        //
        // TODO(diagnostics): a symmetric attach ambiguity is out of
        // scope here. A callee resolved not-effectful could be flipped
        // by an attach from a sibling lazy body (`g <- function()
        // library(shiny); f <- function() reactive({...}`). Detecting it
        // needs the complete set of lazy-context attaches, a post-pass
        // rather than this local ancestor check, so it belongs in the
        // future salsa diagnostics query where this lint should move too.
        if let Some(overwrite_range) = self.is_lazily_shadowed(sym) {
            self.record_lazy_shadow_ambiguity(sym.to_string(), range, overwrite_range);
        }

        Some(effects)
    }

    /// Local resolver for declared effects, mirroring the imports resolver's
    /// `resolve_effects()` method on the cross-file side.
    /// TODO(nse, annotations): always `None` until `declare()` parsing lands.
    fn resolve_local_effects(&self, _name: &str) -> Option<EffectsHandlers> {
        None
    }

    /// Recognize a binding operator (`x %<>% f()`, `x %<~% expr`, `x := expr`)
    /// as an assign effect and build its bindings, or `None` for any other binary
    /// operator.
    pub(super) fn resolve_operator_assign(
        &mut self,
        bin: &RBinaryExpression,
    ) -> Option<Vec<AssignBinding>> {
        let op = bin.operator().ok()?;

        // A binding operator is either a `%...%` (`SPECIAL`, e.g. `%<>%`, where
        // the operator text distinguishes it from `%>%`) or the walrus `:=`
        // (`WALRUS`). Gate on the token kind before consulting the registry so we
        // skip the resolver for ordinary operators like `+`.
        if !matches!(op.kind(), RSyntaxKind::SPECIAL | RSyntaxKind::WALRUS) {
            return None;
        }
        let op_text = op.text_trimmed();

        // Bail early if this operator is not known to have effects annotations
        if !effects::annotates(op_text) {
            return None;
        }

        let handlers = self.resolve_symbol_effects(op_text, bin.syntax().text_trimmed_range())?;

        let bindings = ScanBindings { builder: &*self };
        let ctx = CallContext::with_bindings(&bindings);
        handlers.assign?.resolve(EffectSite::Operator(bin), &ctx)
    }

    /// Detect ambiguities caused by laziness.
    ///
    /// We've recognized an effect for `name` (NSE scope or attach) because it
    /// was locally unbound at the current flow cursor and eager-flow resolution
    /// found one. If we're in a lazy context, that decision could be wrong: an
    /// enclosing scope may bind `name` with a timing we can't pin down, either a
    /// later assignment, or one from another deferred body that could run before
    /// or after us. We detect this ambiguity here so it can be linted.
    ///
    /// Returns the site of the shadowing binding.
    fn is_lazily_shadowed(&self, name: &str) -> Option<TextRange> {
        let mut scope = self.current_scope;
        let mut crossed_lazy = self.scopes[scope].kind.is_lazy();

        while let Some(parent) = self.scopes[scope].parent {
            if crossed_lazy {
                if let Some(range) = self.scope_binding_range(parent, name) {
                    return Some(range);
                }
            }

            if self.scopes[parent].kind.is_lazy() {
                crossed_lazy = true;
            }
            scope = parent;
        }

        None
    }

    fn record_lazy_shadow_ambiguity(
        &mut self,
        name: String,
        call_range: TextRange,
        overwrite_range: TextRange,
    ) {
        self.diagnostics
            .push(SemanticDiagnostic::LazyShadowAmbiguity {
                name,
                call_range,
                overwrite_range,
            });
    }
}
