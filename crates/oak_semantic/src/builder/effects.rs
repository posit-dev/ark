use std::borrow::Cow;

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
use crate::semantic_index::AmbiguityReason;
use crate::semantic_index::ScopeId;
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
    ///   search path, against the attach set in `attached_so_far`.
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
        // carry declared effects that we resolve here).
        if self.scan.bound_so_far.is_bound(sym) {
            return self.resolve_local_effects(sym);
        }

        // Bail early if it is known that no package annotates this name
        // with effects. This speeds up the common case of no known annotations.
        if !effects::annotates(sym) {
            return None;
        }

        // Now check imports since the symbol is locally unbound
        let attached = attach_search_path(
            &self.scan.attached_inherited,
            self.scan.attached_so_far.packages(),
        );
        let effects = self.resolver.resolve_effects(sym, &attached);

        let Some(effects) = effects else {
            // The search path didn't resolve. Probe whether it would have if a
            // dropped attach had survived the join, so we can flag it.
            self.record_conditional_attach_ambiguity(sym, range);
            return None;
        };

        // The callee is unbound by any eager binding, so its effect
        // holds. If a lazy-crossed ancestor binds it whole-scope, that
        // binding's timing relative to this deferred body is
        // undetermined, so the decision is a guess. Flag it.
        if let Some(overwrite_range) = self.is_lazily_shadowed(sym) {
            self.record_lazy_shadow_ambiguity(sym.to_string(), range, overwrite_range);
        }

        Some(effects)
    }

    /// Local resolver for declared effects, mirroring the imports resolver's
    /// `resolve_effects()` method on the cross-file side.
    ///
    /// TODO(nse, annotations): Resolve effects declare()'d on local functions.
    ///
    /// TODO(nse, inference): Infer effects from local function bodies. Calling
    /// `g()` should apply the attach in `g <- function() library(shiny)`. Mutual
    /// recursion needs a fixed point.
    fn resolve_local_effects(&self, _name: &str) -> Option<EffectsHandlers> {
        None
    }

    /// Resolve a binding operator's definitions.
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
    /// found an effect. If we're in a lazy context, that decision could be
    /// wrong: an enclosing scope may bind `name` with a timing we can't pin
    /// down, either a later assignment, or one from another deferred body that
    /// could run before or after us. We detect this ambiguity here so it can be
    /// linted.
    ///
    /// Returns the site of the shadowing binding.
    fn is_lazily_shadowed(&self, name: &str) -> Option<TextRange> {
        let mut open_scopes = self.scan.open_scopes.iter().rev();
        match open_scopes.next() {
            // Search the body's ancestors from the inside out for a binding of
            // `name` we can't order against the body (see the doc above). Here
            // the body is the innermost open scope, e.g. a `local()` /
            // `on_load()` body the scan entered before the walk gave it an
            // arena scope. Its ancestors are the frames beneath it, then the
            // arena scopes from `current_scope` out (included). The `None` arm
            // is the mirror case, where `current_scope` is the body itself and
            // the walk starts at its parent.
            Some(body) => {
                let mut crossed_lazy = body.kind.is_lazy();
                for scope in open_scopes {
                    if crossed_lazy {
                        if let Some(range) = scope.bindings.binding_range(name) {
                            return Some(range);
                        }
                    }
                    if scope.kind.is_lazy() {
                        crossed_lazy = true;
                    }
                }

                self.lazy_shadow_in_arena(name, Some(self.current_scope), crossed_lazy)
            },

            // No frames: the body is `current_scope` itself (a function or
            // other lazy context like `reactive()`, scanned at walk time), so
            // its ancestors start at its parent.
            None => self.lazy_shadow_in_arena(
                name,
                self.scopes[self.current_scope].parent,
                self.scopes[self.current_scope].kind.is_lazy(),
            ),
        }
    }

    /// Walk arena scopes outward from `start`, returning the first that binds
    /// `name` after a lazy boundary has been crossed.
    fn lazy_shadow_in_arena(
        &self,
        name: &str,
        start: Option<ScopeId>,
        mut crossed_lazy: bool,
    ) -> Option<TextRange> {
        let mut scope = start;

        while let Some(s) = scope {
            if crossed_lazy {
                if let Some(range) = self.scope_binding_range(s, name) {
                    return Some(range);
                }
            }
            if self.scopes[s].kind.is_lazy() {
                crossed_lazy = true;
            }
            scope = self.scopes[s].parent;
        }

        None
    }

    fn record_lazy_shadow_ambiguity(
        &mut self,
        name: String,
        call_range: TextRange,
        overwrite_range: TextRange,
    ) {
        self.diagnostics.push(SemanticDiagnostic::AmbiguousEffect {
            name,
            call_range,
            reason: AmbiguityReason::LazyShadow { overwrite_range },
        });
    }

    /// Probe whether `sym`'s effect still resolves after a conditional
    /// attach. If the case, we record the ambiguity for diagnostics.
    ///
    /// This handles the eager case, where the dropped attach and the callee are
    /// both reachable from the same scan. What's still open is the lazy-sibling
    /// case (`g <- function() library(shiny); f <- function() reactive({...})`),
    /// which needs the complete set of lazy-context attaches from a post-pass,
    /// not this call-site probe. That belongs in the future salsa diagnostics
    /// query where this lint family should move too.
    fn record_conditional_attach_ambiguity(&mut self, sym: &str, call_range: TextRange) {
        // A package in `attached_anywhere` but off the search path means it was
        // dropped at a branch or loop join
        let search_path = attach_search_path(
            &self.scan.attached_inherited,
            self.scan.attached_so_far.packages(),
        );
        let dropped: Vec<(String, TextRange)> = self
            .scan
            .attached_anywhere
            .iter()
            .filter(|(package, _)| !search_path.contains(package))
            .cloned()
            .collect();

        // Probe one package at a time, most recent first, so the diagnostic
        // mentions the attach that would actually have carried the effect.
        for (package, attach_range) in dropped.into_iter().rev() {
            if self
                .resolver
                .resolve_effects(sym, std::slice::from_ref(&package))
                .is_none()
            {
                continue;
            }

            self.diagnostics.push(SemanticDiagnostic::AmbiguousEffect {
                name: sym.to_string(),
                call_range,
                reason: AmbiguityReason::ConditionalAttach {
                    package,
                    attach_range,
                },
            });
            return;
        }
    }
}

/// The packages seen in a scan unit: what it inherited at its definition point,
/// then the eager linear set. Used for resolution of effect annotations within
/// that scan unit.
///
/// The two halves only differ for a lazy body defined inside a branch that
/// attached: the join dropped that package from `attached_so_far`, and the
/// inherited half is what keeps it reachable.
pub(super) fn attach_search_path<'a>(
    inherited: &'a [String],
    so_far: &'a [String],
) -> Cow<'a, [String]> {
    if inherited.is_empty() {
        return Cow::Borrowed(so_far);
    }

    let mut path = inherited.to_vec();
    path.extend_from_slice(so_far);
    Cow::Owned(path)
}
