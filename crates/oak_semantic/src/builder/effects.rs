use std::borrow::Cow;

use aether_syntax::AnyRExpression;
use aether_syntax::RBinaryExpression;
use aether_syntax::RCall;
use aether_syntax::RNamespaceExpression;
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
use crate::effects::CalleeOrigin;
use crate::effects::CalleeResolution;
use crate::effects::CalleeUncertainty;
use crate::effects::DroppedAttach;
use crate::effects::EffectSite;
use crate::effects::Effects;
use crate::effects::FunctionHandlers;
use crate::resolver::ImportsResolver;
use crate::semantic_index::AmbiguityReason;
use crate::semantic_index::ScopeId;
use crate::semantic_index::SemanticDiagnostic;

impl<R: ImportsResolver> SemanticIndexBuilder<R> {
    pub(super) fn resolve_effects(&mut self, call: &RCall) -> Option<Effects> {
        let resolution = self.resolve_callee(call);
        if let Some(reason) = resolution.ambiguity(FunctionHandlers::has_effects) {
            self.record_call_ambiguity(call, reason);
        }
        let handlers = resolution.handlers?.effects;

        let mut bindings = ScanBindings { builder: self };
        let mut ctx = CallContext::new(&mut bindings);

        let arguments = handlers
            .arguments
            .and_then(|handler| handler.resolve(call, &mut ctx));
        let attach = handlers
            .attach
            .and_then(|handler| handler.resolve(call, &mut ctx));
        let source = handlers
            .source
            .and_then(|handler| handler.resolve(call, &mut ctx));
        let assign = handlers
            .assign
            .and_then(|handler| handler.resolve(EffectSite::Call(call), &mut ctx));

        Some(Effects {
            arguments,
            attach,
            source,
            assign,
        })
    }

    /// Resolve during the scan, not the walk, because bare names depend on
    /// flow-precise bindings and attachments at the call site. Qualified names
    /// bypass those bindings, and the `sourceDir()` idiom overrides the handlers
    /// that name lookup finds.
    ///
    /// Lookup returns uncertainty without emitting diagnostics. Effect handling
    /// and static evaluation each use [`CalleeResolution::ambiguity()`] to
    /// report only uncertainty relevant to the handlers they need.
    pub(super) fn resolve_callee(&mut self, call: &RCall) -> CalleeResolution {
        let Ok(func) = call.function() else {
            return CalleeResolution::settled(CalleeOrigin::Dynamic, None);
        };

        match &func {
            AnyRExpression::RIdentifier(ident) => {
                let name = ident.name_text();
                let resolution = self.resolve_symbol(&name);

                // Local bindings must not disable the `sourceDir()` idiom,
                // which is usually defined in the file itself. Keep the lookup
                // origin, but discard its uncertainty because the idiom's
                // handlers apply regardless of which binding the name resolves to.
                match effects::source_dir_idiom(&name) {
                    Some(handlers) => CalleeResolution::settled(resolution.origin, Some(*handlers)),
                    None => resolution,
                }
            },

            AnyRExpression::RNamespaceExpression(ns_expr) => CalleeResolution::settled(
                CalleeOrigin::Qualified,
                self.resolve_qualified_effects(ns_expr),
            ),

            _ => CalleeResolution::settled(CalleeOrigin::Dynamic, None),
        }
    }

    fn resolve_qualified_effects(
        &mut self,
        ns_expr: &RNamespaceExpression,
    ) -> Option<FunctionHandlers> {
        let pkg = ns_expr.left().ok()?.identifier_text()?;
        let func_name = ns_expr.right().ok()?.identifier_text()?;

        if !effects::annotates(&func_name) {
            return None;
        }

        self.resolver.resolve_qualified_effects(&pkg, &func_name)
    }

    /// Local bindings take precedence over both dropped attaches and lazy
    /// ancestor bindings. Calls and binding operators share this lookup.
    fn resolve_symbol(&mut self, sym: &str) -> CalleeResolution {
        // First check for a local definition (which in the future may
        // carry declared effects that we resolve here).
        if self.scan.bound_so_far.is_bound(sym) {
            return CalleeResolution::settled(CalleeOrigin::Local, self.resolve_local_effects(sym));
        }

        // Bail early if it is known that no package annotates this name
        // with effects. This speeds up the common case of no known annotations.
        if !effects::annotates(sym) {
            return CalleeResolution::settled(CalleeOrigin::SearchPath, None);
        }

        // Now check imports since the symbol is locally unbound
        let attached = attach_search_path(
            &self.scan.attached_inherited,
            self.scan.attached_so_far.packages(),
        );
        let handlers = self.resolver.resolve_effects(sym, &attached);

        let uncertainty = match handlers {
            Some(_) => self
                .is_lazily_shadowed(sym)
                .map(|overwrite_range| CalleeUncertainty::LazyShadow { overwrite_range }),
            None => self.conditional_attach_uncertainty(sym),
        };

        CalleeResolution::new(CalleeOrigin::SearchPath, handlers, uncertainty)
    }

    pub(super) fn record_call_ambiguity(&mut self, call: &RCall, reason: AmbiguityReason) {
        let Ok(AnyRExpression::RIdentifier(ident)) = call.function() else {
            return;
        };
        self.record_ambiguity(
            &ident.name_text(),
            call.syntax().text_trimmed_range(),
            reason,
        );
    }

    fn record_ambiguity(&mut self, name: &str, call_range: TextRange, reason: AmbiguityReason) {
        self.diagnostics.push(SemanticDiagnostic::AmbiguousEffect {
            name: name.to_string(),
            call_range,
            reason,
        });
    }

    /// Local resolver for declared effects, mirroring the imports resolver's
    /// `resolve_effects()` method on the cross-file side.
    ///
    /// TODO(nse, annotations): Resolve effects declare()'d on local functions.
    ///
    /// TODO(nse, inference): Infer effects from local function bodies. Calling
    /// `g()` should apply the attach in `g <- function() library(shiny)`. Mutual
    /// recursion needs a fixed point.
    fn resolve_local_effects(&self, _name: &str) -> Option<FunctionHandlers> {
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

        let resolution = self.resolve_symbol(op_text);
        if let Some(reason) = resolution.ambiguity(FunctionHandlers::has_effects) {
            self.record_ambiguity(op_text, bin.syntax().text_trimmed_range(), reason);
        }
        let handlers = resolution.handlers?.effects;

        let mut bindings = ScanBindings { builder: self };
        let mut ctx = CallContext::new(&mut bindings);
        handlers
            .assign?
            .resolve(EffectSite::Operator(bin), &mut ctx)
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

    /// Probe dropped attaches independently, in reverse attach order, so each
    /// consumer can report the most recent candidate with the handler it needs.
    /// This does not reconstruct the runtime search path.
    ///
    /// The probe sees only attaches reachable from the callee's scan. Attaches
    /// in sibling lazy bodies are not in that set, even if those bodies could
    /// run before the callee.
    fn conditional_attach_uncertainty(&mut self, sym: &str) -> Option<CalleeUncertainty> {
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

        let candidates: Vec<DroppedAttach> = dropped
            .into_iter()
            .rev()
            .filter_map(|(package, attach_range)| {
                let handlers = self
                    .resolver
                    .resolve_effects(sym, std::slice::from_ref(&package))?;
                Some(DroppedAttach {
                    package,
                    attach_range,
                    handlers,
                })
            })
            .collect();

        (!candidates.is_empty()).then_some(CalleeUncertainty::ConditionalAttach(candidates))
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
