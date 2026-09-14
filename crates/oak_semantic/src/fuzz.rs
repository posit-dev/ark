//! Program model and renderer for generated R fuzz inputs.
//!
//! This module renders candidate effects. The production builder decides whether
//! the rendered code has an effect.
//!
//! Mutually recursive with [`crate::effects::fuzz`]: an effect's body
//! holds statements, and a statement can hold an effect.

use crate::effects::fuzz::callee;
use crate::effects::fuzz::render_effect;
use crate::effects::fuzz::Callee;
use crate::effects::fuzz::EffectRecipe;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Program {
    pub statements: Vec<Stmt>,
}

pub type Block = Vec<Stmt>;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Stmt {
    /// Binding a callee name shadows that callee for later bare calls.
    Bind {
        name: String,
        value: Expr,
    },
    Expr(Expr),
    Effect {
        recipe: EffectRecipe,
        invocation: Invocation,
    },
}

impl Stmt {
    pub fn bind(name: &str, value: Expr) -> Stmt {
        Stmt::Bind {
            name: name.to_string(),
            value,
        }
    }

    pub fn use_of(name: &str) -> Stmt {
        Stmt::Expr(Expr::Ident(name.to_string()))
    }

    pub fn effect(recipe: EffectRecipe, invocation: Invocation) -> Stmt {
        Stmt::Effect { recipe, invocation }
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Expr {
    Num(i32),
    Null,
    Ident(String),

    Function {
        body: Block,
    },

    Call {
        name: String,
    },
    /// `bquote()` evaluation hole. A `Block` rather than a bare `Expr` because
    /// effects only exist as `Stmt::Effect`, so a hole needs a block to hold
    /// an escaped effect like `.(source("b.R"))`.
    Hole(Block),
}

impl Expr {
    pub fn function(body: Block) -> Expr {
        Expr::Function { body }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Invocation {
    Bare,
    Qualified,
}

pub struct Rendered {
    /// Byte offset of the first call-shaped effect callee, including nested
    /// effects. Qualified calls point to the function name, not the package.
    pub first_call: Option<usize>,
    /// Byte offset of the last identifier. Never points inside a string literal.
    pub last_identifier: Option<usize>,
    pub text: String,
}

impl Program {
    pub fn render(&self) -> Rendered {
        let mut renderer = Renderer::default();
        for stmt in &self.statements {
            renderer.line(stmt);
        }
        renderer.finish()
    }

    /// Effect callees in source order, including nested bodies, for shadow selection.
    pub fn callees(&self) -> Vec<Callee> {
        let mut callees = Vec::new();
        collect_block_callees(&self.statements, &mut callees);
        callees
    }
}

fn collect_block_callees(block: &Block, callees: &mut Vec<Callee>) {
    for stmt in block {
        collect_stmt_callees(stmt, callees);
    }
}

fn collect_stmt_callees(stmt: &Stmt, callees: &mut Vec<Callee>) {
    match stmt {
        Stmt::Bind { value, .. } | Stmt::Expr(value) => collect_expr_callees(value, callees),
        Stmt::Effect { recipe, .. } => {
            callees.push(callee(recipe));
            collect_effect_body_callees(recipe, callees);
        },
    }
}

fn collect_effect_body_callees(recipe: &EffectRecipe, callees: &mut Vec<Callee>) {
    match recipe {
        EffectRecipe::Eval { body, .. } |
        EffectRecipe::Quote { body } |
        EffectRecipe::QuoteHoles { body } |
        EffectRecipe::Substitute { body } => collect_block_callees(body, callees),
        EffectRecipe::Rebind { value, .. } | EffectRecipe::Assign { value, .. } => {
            collect_expr_callees(value, callees)
        },
        EffectRecipe::Source { .. } | EffectRecipe::Attach { .. } => {},
    }
}

fn collect_expr_callees(expr: &Expr, callees: &mut Vec<Callee>) {
    match expr {
        Expr::Function { body } | Expr::Hole(body) => collect_block_callees(body, callees),
        Expr::Num(_) | Expr::Null | Expr::Ident(_) | Expr::Call { .. } => {},
    }
}

/// Renders program text while recording offsets, avoiding a search of completed text.
///
/// [`crate::effects::fuzz::render_effect`] writes to the same output and offset state.
#[derive(Default)]
pub(crate) struct Renderer {
    out: String,
    depth: usize,
    first_call: Option<usize>,
    last_identifier: Option<usize>,
}

impl Renderer {
    fn finish(self) -> Rendered {
        Rendered {
            text: self.out,
            first_call: self.first_call,
            last_identifier: self.last_identifier,
        }
    }

    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.out.push_str("  ");
        }
    }

    pub(crate) fn push_str(&mut self, text: &str) {
        self.out.push_str(text);
    }

    pub(crate) fn push_identifier(&mut self, text: &str) {
        self.last_identifier = Some(self.out.len());
        self.out.push_str(text);
    }

    pub(crate) fn push_call_identifier(&mut self, text: &str) {
        if self.first_call.is_none() {
            self.first_call = Some(self.out.len());
        }
        self.push_name(text);
    }

    pub(crate) fn push_name(&mut self, name: &str) {
        if is_syntactic_name(name) {
            self.push_identifier(name);
            return;
        }
        // A backtick or backslash inside the name would close the quoting early.
        let escaped = name.replace('\\', "\\\\").replace('`', "\\`");
        self.push_identifier(&format!("`{escaped}`"));
    }

    fn line(&mut self, stmt: &Stmt) {
        self.indent();
        self.inline(stmt);
        self.out.push('\n');
    }

    fn inline(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Bind { name, value } => {
                self.push_name(name);
                self.out.push_str(" <- ");
                self.expr(value);
            },
            Stmt::Expr(expr) => self.expr(expr),
            Stmt::Effect { recipe, invocation } => render_effect(self, recipe, *invocation),
        }
    }

    pub(crate) fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Num(n) => self.out.push_str(&n.to_string()),
            Expr::Null => self.out.push_str("NULL"),
            Expr::Ident(name) => self.push_name(name),
            Expr::Function { body } => {
                self.out.push_str("function(...) ");
                self.block(body);
            },
            Expr::Call { name } => {
                self.push_name(name);
                self.out.push_str("()");
            },
            Expr::Hole(body) => {
                self.out.push_str(".(");
                self.block(body);
                self.out.push(')');
            },
        }
    }

    /// Renders an empty block as `NULL`, a single statement inline, and longer
    /// blocks with braces. This matches R function and non-standard-evaluation bodies.
    pub(crate) fn block(&mut self, block: &Block) {
        match block.as_slice() {
            [] => self.out.push_str("NULL"),
            [only] => self.inline(only),
            stmts => {
                self.out.push_str("{\n");
                self.depth += 1;
                for stmt in stmts {
                    self.line(stmt);
                }
                self.depth -= 1;
                self.indent();
                self.out.push('}');
            },
        }
    }
}

/// R's reserved words: syntactically valid identifiers R's parser never treats
/// as a name.
const RESERVED_WORDS: &[&str] = &[
    "if",
    "else",
    "repeat",
    "while",
    "function",
    "for",
    "next",
    "break",
    "TRUE",
    "FALSE",
    "NULL",
    "Inf",
    "NaN",
    "NA",
    "NA_integer_",
    "NA_real_",
    "NA_character_",
];

/// Whether `name` can render bare instead of backtick-quoted in R.
fn is_syntactic_name(name: &str) -> bool {
    if RESERVED_WORDS.contains(&name) {
        return false;
    }

    let mut chars = name.chars();
    match chars.next() {
        // A leading dot followed by a digit lexes as a number (`.1`), not a name.
        Some('.') if name.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) => return false,
        Some(first) if first.is_ascii_alphabetic() || first == '.' => {},
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
}
