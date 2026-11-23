//! The call whose parentheses contain the cursor, as written in the document.
//!
//! Call extraction is independent of completion rules. Signature help can
//! use the same [CallSite] and match its active argument to a callee's
//! formals with [CallSite::active_formal()].

use aether_syntax::AnyRArgumentName;
use aether_syntax::AnyRExpression;
use aether_syntax::RArgument;
use aether_syntax::RCall;
use aether_syntax::RSyntaxKind;
use aether_syntax::RSyntaxNode;
use aether_syntax::RSyntaxToken;
use biome_rowan::AstNode;
use biome_rowan::AstSeparatedList;
use biome_rowan::Direction;
use biome_rowan::TextRange;
use biome_rowan::TextSize;
use oak_core::syntax_ext::delimiters_contain;
use oak_core::syntax_ext::is_name_token;
use oak_core::syntax_ext::last_token_before;
use oak_core::syntax_ext::touches;
use oak_core::syntax_ext::RIdentifierExt;

/// A call around the cursor, with owned text and ranges.
///
/// R calls mix named and unnamed arguments, as in `foo(a = 1, b, c = 2, d)`,
/// so the active argument's index alone doesn't identify the active formal.
/// `arguments` keeps every argument's name so that the caller can skip the
/// formals supplied by name, before and after the cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct CallSite {
    /// Source text of the callee, `f` in `f(x)` or `pkg::f` in `pkg::f(x)`,
    /// which a session evaluates.
    pub callee: String,
    /// Range of the callee, which identifies it for
    /// `oak_ide::callee_description()`.
    pub callee_range: TextRange,
    /// The arguments in the tree, in order. An empty argument, as after the
    /// comma in `f(1, )`, has neither name nor value. `f()` has none.
    pub arguments: Vec<CallArgument>,
    /// Index of the argument containing the cursor, counted by commas between
    /// the opening parenthesis and the cursor. Equals `arguments.len()` in
    /// `f(|)`, whose argument list is empty.
    pub active: usize,
    /// Which part of the active argument the cursor is in.
    pub position: ArgumentPosition,
}

impl CallSite {
    pub fn active_argument(&self) -> Option<&CallArgument> {
        self.arguments.get(self.active)
    }

    /// Source text of the first argument when it is supplied without a name,
    /// the object a session evaluates for S3 dispatch.
    pub fn dispatch_argument(&self) -> Option<&str> {
        let first = self.arguments.first()?;
        if first.name.is_some() {
            return None;
        }
        first.value.as_deref()
    }

    /// Approximates R's argument matching to find the active formal's index.
    ///
    /// An active argument supplied by a name among `formals` matches that
    /// formal. Otherwise the arguments without a name before the active one
    /// fill the formals in order, skipping those supplied by name anywhere in
    /// the call, and the active argument takes the next one. Partial name
    /// matching and the formals after `...`, which only match by name, are
    /// not modelled. `None` when the positional arguments use up the formals.
    pub fn active_formal(&self, formals: &[impl AsRef<str>]) -> Option<usize> {
        let active_name = self
            .active_argument()
            .and_then(|argument| argument.name.as_deref());
        if let Some(name) = active_name {
            if let Some(index) = formals.iter().position(|formal| formal.as_ref() == name) {
                return Some(index);
            }
        }

        let mut positional = self.arguments[..self.active.min(self.arguments.len())]
            .iter()
            .filter(|argument| argument.name.is_none())
            .count();

        for (index, formal) in formals.iter().enumerate() {
            let supplied_by_name = self
                .arguments
                .iter()
                .any(|argument| argument.name.as_deref() == Some(formal.as_ref()));
            if supplied_by_name {
                continue;
            }
            if positional > 0 {
                positional -= 1;
                continue;
            }
            return Some(index);
        }

        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallArgument {
    /// The name in `name = value`, without backquotes or quotes, as `a` in
    /// ``f(`a` = 1)`` or `f("a" = 1)`, so that it compares equal to the name
    /// of a formal.
    pub name: Option<String>,
    /// Source text of the value.
    pub value: Option<String>,
    pub range: TextRange,
}

/// Where the cursor is in `f(name = value)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ArgumentPosition {
    /// Before an argument's value, as in `f(|)` or `f(x, |)`, or within a
    /// lone name, as in `f(ab|)`, which could become a name or a value.
    Name,
    /// After the `=` of `name = value`, or after the start of a value that
    /// isn't a lone name, as in `f(x + |)`.
    Value,
}

/// The call whose parentheses contain `offset`, within the innermost braces
/// that contain it.
pub fn call_site_at(root: &RSyntaxNode, offset: TextSize) -> Option<CallSite> {
    let last = last_token_before(root, offset)?;
    let call = enclosing_call(&last, offset)?;
    call_site(&call, offset)
}

/// Like [call_site_at()], from the last token before the cursor.
///
/// Delimiters, not the covering node, determine the enclosing call. In
/// `g(f()|)` the last token is `f`'s `)`, but the enclosing call is `g`.
/// Braces end the search, so in `f({ x; | })` the cursor is in a statement,
/// not in an argument of `f`.
pub fn enclosing_call(last: &RSyntaxToken, offset: TextSize) -> Option<RCall> {
    for node in last.parent()?.ancestors() {
        match node.kind() {
            RSyntaxKind::R_BRACED_STATEMENTS if delimiters_contain(&node, offset) => return None,
            RSyntaxKind::R_CALL_ARGUMENTS if delimiters_contain(&node, offset) => {
                return node.parent().and_then(RCall::cast);
            },
            _ => {},
        }
    }
    None
}

pub fn call_site(call: &RCall, offset: TextSize) -> Option<CallSite> {
    let callee = call.function().ok()?;
    let list = call.arguments().ok()?.items();

    let arguments = list
        .iter()
        .filter_map(|argument| argument.ok())
        .map(|argument| call_argument(&argument))
        .collect();

    let active = list
        .separators()
        .filter_map(|separator| separator.ok())
        .filter(|comma| comma.text_trimmed_range().end() <= offset)
        .count();

    let position = match list.iter().nth(active).and_then(|argument| argument.ok()) {
        Some(argument) => argument_position(&argument, offset),
        None => ArgumentPosition::Name,
    };

    Some(CallSite {
        callee: callee.syntax().text_trimmed().to_string(),
        callee_range: callee.syntax().text_trimmed_range(),
        arguments,
        active,
        position,
    })
}

fn call_argument(argument: &RArgument) -> CallArgument {
    let name = argument
        .name_clause()
        .and_then(|clause| clause.name().ok())
        .map(|name| argument_name(&name));
    let value = argument.value().map(|value| expression_text(&value));

    CallArgument {
        name,
        value,
        range: argument.syntax().text_trimmed_range(),
    }
}

/// Uses the same name normalization as `oak_semantic` so argument names
/// compare equal to formals. Backquotes and quotes are removed, as for `a`
/// in ``f(`a` = 1)`` or `f("a" = 1)`. Other argument-name kinds retain
/// their source text.
fn argument_name(name: &AnyRArgumentName) -> String {
    let unquoted = match name {
        AnyRArgumentName::RIdentifier(identifier) => Some(identifier.name_text()),
        AnyRArgumentName::RStringValue(string) => {
            string.inner_string_text().ok().map(|text| text.to_string())
        },
        AnyRArgumentName::RDotDotI(_) |
        AnyRArgumentName::RDots(_) |
        AnyRArgumentName::RNullExpression(_) => None,
    };
    match unquoted {
        Some(name) => name,
        None => name.syntax().text_trimmed().to_string(),
    }
}

fn argument_position(argument: &RArgument, offset: TextSize) -> ArgumentPosition {
    let after_equal = argument
        .name_clause()
        .and_then(|clause| clause.eq_token().ok())
        .is_some_and(|equal| equal.text_trimmed_range().end() <= offset);
    if after_equal {
        return ArgumentPosition::Value;
    }

    let mut before_cursor = argument
        .syntax()
        .descendants_tokens(Direction::Next)
        .filter(|token| token.text_trimmed_range().start() < offset);

    match (before_cursor.next(), before_cursor.next()) {
        (None, _) => ArgumentPosition::Name,
        (Some(only), None) if is_name_token(&only) && touches(&only, offset) => {
            ArgumentPosition::Name
        },
        _ => ArgumentPosition::Value,
    }
}

fn expression_text(expression: &AnyRExpression) -> String {
    expression.syntax().text_trimmed().to_string()
}
