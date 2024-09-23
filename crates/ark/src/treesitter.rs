use tree_sitter::Node;

use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::rope::RopeExt;

#[derive(Debug, PartialEq)]
pub enum NodeType {
    Program,
    FunctionDefinition,
    Parameters,
    Parameter,
    IfStatement,
    ForStatement,
    WhileStatement,
    RepeatStatement,
    BracedExpression,
    ParenthesizedExpression,
    Call,
    Subset,
    Subset2,
    Arguments,
    Argument,
    UnaryOperator(UnaryOperatorType),
    BinaryOperator(BinaryOperatorType),
    ExtractOperator(ExtractOperatorType),
    NamespaceOperator(NamespaceOperatorType),
    Integer,
    Complex,
    Float,
    String,
    StringContent,
    EscapeSequence,
    Identifier,
    DotDotI,
    Dots,
    Return,
    Next,
    Break,
    True,
    False,
    Null,
    Inf,
    Nan,
    Na(NaType),
    Comment,
    Comma,
    Error,
    Anonymous(String),
}

fn node_type(x: &Node) -> NodeType {
    match x.kind() {
        "program" => NodeType::Program,
        "function_definition" => NodeType::FunctionDefinition,
        "parameters" => NodeType::Parameters,
        "parameter" => NodeType::Parameter,
        "if_statement" => NodeType::IfStatement,
        "for_statement" => NodeType::ForStatement,
        "while_statement" => NodeType::WhileStatement,
        "repeat_statement" => NodeType::RepeatStatement,
        "braced_expression" => NodeType::BracedExpression,
        "parenthesized_expression" => NodeType::ParenthesizedExpression,
        "call" => NodeType::Call,
        "subset" => NodeType::Subset,
        "subset2" => NodeType::Subset2,
        "arguments" => NodeType::Arguments,
        "argument" => NodeType::Argument,
        "unary_operator" => NodeType::UnaryOperator(unary_operator_type(x)),
        "binary_operator" => NodeType::BinaryOperator(binary_operator_type(x)),
        "extract_operator" => NodeType::ExtractOperator(extract_operator_type(x)),
        "namespace_operator" => NodeType::NamespaceOperator(namespace_operator_type(x)),
        "integer" => NodeType::Integer,
        "complex" => NodeType::Complex,
        "float" => NodeType::Float,
        "string" => NodeType::String,
        "string_content" => NodeType::StringContent,
        "escape_sequence" => NodeType::EscapeSequence,
        "identifier" => NodeType::Identifier,
        "dot_dot_i" => NodeType::DotDotI,
        "dots" => NodeType::Dots,
        "return" => NodeType::Return,
        "next" => NodeType::Next,
        "break" => NodeType::Break,
        "true" => NodeType::True,
        "false" => NodeType::False,
        "null" => NodeType::Null,
        "inf" => NodeType::Inf,
        "nan" => NodeType::Nan,
        "na" => NodeType::Na(na_type(x)),
        "comment" => NodeType::Comment,
        "comma" => NodeType::Comma,
        "ERROR" => NodeType::Error,
        anonymous => NodeType::Anonymous(anonymous.to_string()),
    }
}

#[derive(Debug, PartialEq)]
pub enum UnaryOperatorType {
    /// `?`
    Help,
    /// `~`
    Tilde,
    /// `!`
    Not,
    /// `+`
    Plus,
    /// `-`
    Minus,
}

fn unary_operator_type(x: &Node) -> UnaryOperatorType {
    let x = x.child_by_field_name("operator").unwrap();

    match x.kind() {
        "?" => UnaryOperatorType::Help,
        "~" => UnaryOperatorType::Tilde,
        "!" => UnaryOperatorType::Not,
        "+" => UnaryOperatorType::Plus,
        "-" => UnaryOperatorType::Minus,
        _ => panic!("Unknown `unary_operator` kind {}.", x.kind()),
    }
}

#[derive(Debug, PartialEq)]
pub enum BinaryOperatorType {
    /// `?`
    Help,
    /// `~`
    Tilde,
    /// `<-`
    LeftAssignment,
    /// `<<-`
    LeftSuperAssignment,
    /// `:=`
    WalrusAssignment,
    /// `->`
    RightAssignment,
    /// `->>`
    RightSuperAssignment,
    /// `=`
    EqualsAssignment,
    /// `|`
    Or,
    /// `&`
    And,
    /// `||`
    Or2,
    /// `&&`
    And2,
    /// `<`
    LessThan,
    /// `<=`
    LessThanOrEqualTo,
    /// `>`
    GreaterThan,
    /// `>=`
    GreaterThanOrEqualTo,
    /// `==`
    Equal,
    /// `!=`
    NotEqual,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Multiply,
    /// `/`
    Divide,
    /// `^` or `**`
    Exponentiate,
    /// Infix operators, like `%>%`
    Special,
    /// `|>`
    Pipe,
    /// `:`
    Colon,
}

fn binary_operator_type(x: &Node) -> BinaryOperatorType {
    let x = x.child_by_field_name("operator").unwrap();

    match x.kind() {
        "?" => BinaryOperatorType::Help,
        "~" => BinaryOperatorType::Tilde,
        "<-" => BinaryOperatorType::LeftAssignment,
        "<<-" => BinaryOperatorType::LeftSuperAssignment,
        ":=" => BinaryOperatorType::WalrusAssignment,
        "->" => BinaryOperatorType::RightAssignment,
        "->>" => BinaryOperatorType::RightSuperAssignment,
        "=" => BinaryOperatorType::EqualsAssignment,
        "|" => BinaryOperatorType::Or,
        "&" => BinaryOperatorType::And,
        "||" => BinaryOperatorType::Or2,
        "&&" => BinaryOperatorType::And2,
        "<" => BinaryOperatorType::LessThan,
        "<=" => BinaryOperatorType::LessThanOrEqualTo,
        ">" => BinaryOperatorType::GreaterThan,
        ">=" => BinaryOperatorType::GreaterThanOrEqualTo,
        "==" => BinaryOperatorType::Equal,
        "!=" => BinaryOperatorType::NotEqual,
        "+" => BinaryOperatorType::Plus,
        "-" => BinaryOperatorType::Minus,
        "*" => BinaryOperatorType::Multiply,
        "/" => BinaryOperatorType::Divide,
        "^" => BinaryOperatorType::Exponentiate,
        "**" => BinaryOperatorType::Exponentiate,
        "special" => BinaryOperatorType::Special,
        "|>" => BinaryOperatorType::Pipe,
        ":" => BinaryOperatorType::Colon,
        _ => panic!("Unknown `binary_operator` kind {}.", x.kind()),
    }
}

#[derive(Debug, PartialEq)]
pub enum ExtractOperatorType {
    /// `$`
    Dollar,
    /// `@`
    At,
}

fn extract_operator_type(x: &Node) -> ExtractOperatorType {
    let x = x.child_by_field_name("operator").unwrap();

    match x.kind() {
        "$" => ExtractOperatorType::Dollar,
        "@" => ExtractOperatorType::At,
        _ => panic!("Unknown `extract_operator` kind {}.", x.kind()),
    }
}

#[derive(Debug, PartialEq)]
pub enum NamespaceOperatorType {
    /// `::`
    External,
    /// `:::`
    Internal,
}

fn namespace_operator_type(x: &Node) -> NamespaceOperatorType {
    let x = x.child_by_field_name("operator").unwrap();

    match x.kind() {
        "::" => NamespaceOperatorType::External,
        ":::" => NamespaceOperatorType::Internal,
        _ => panic!("Unknown `namespace_operator` kind {}.", x.kind()),
    }
}

#[derive(Debug, PartialEq)]
pub enum NaType {
    /// `NA`
    Logical,
    /// `NA_integer_`
    Integer,
    /// `NA_real_`
    Double,
    /// `NA_complex_`
    Complex,
    /// `NA_character_`
    Character,
}

fn na_type(x: &Node) -> NaType {
    let x = x.child(0).unwrap();

    match x.kind() {
        "NA" => NaType::Logical,
        "NA_integer_" => NaType::Integer,
        "NA_real_" => NaType::Double,
        "NA_complex_" => NaType::Complex,
        "NA_character_" => NaType::Character,
        _ => panic!("Unknown `na` kind {}.", x.kind()),
    }
}

pub trait NodeTypeExt: Sized {
    fn node_type(&self) -> NodeType;

    fn is_program(&self) -> bool;
    fn is_identifier(&self) -> bool;
    fn is_string(&self) -> bool;
    fn is_identifier_or_string(&self) -> bool;
    fn is_keyword(&self) -> bool;
    fn is_call(&self) -> bool;
    fn is_subset(&self) -> bool;
    fn is_subset2(&self) -> bool;
    fn is_comment(&self) -> bool;
    fn is_braced_expression(&self) -> bool;
    fn is_function_definition(&self) -> bool;
    fn is_if_statement(&self) -> bool;
    fn is_argument(&self) -> bool;
    fn is_arguments(&self) -> bool;
    fn is_namespace_operator(&self) -> bool;
    fn is_namespace_internal_operator(&self) -> bool;
    fn is_unary_operator(&self) -> bool;
    fn is_binary_operator(&self) -> bool;
    fn is_native_pipe_operator(&self) -> bool;
    fn is_magrittr_pipe_operator(&self, contents: &ropey::Rope) -> anyhow::Result<bool>;
    fn is_pipe_operator(&self, contents: &ropey::Rope) -> anyhow::Result<bool>;
}

impl NodeTypeExt for Node<'_> {
    fn node_type(&self) -> NodeType {
        node_type(self)
    }

    fn is_program(&self) -> bool {
        self.node_type() == NodeType::Program
    }

    fn is_identifier(&self) -> bool {
        self.node_type() == NodeType::Identifier
    }

    fn is_string(&self) -> bool {
        self.node_type() == NodeType::String
    }

    // This combination is particularly common
    fn is_identifier_or_string(&self) -> bool {
        matches!(self.node_type(), NodeType::Identifier | NodeType::String)
    }

    fn is_keyword(&self) -> bool {
        matches!(
            self.node_type(),
            NodeType::Return |
                NodeType::Next |
                NodeType::Break |
                NodeType::True |
                NodeType::False |
                NodeType::Null |
                NodeType::Inf |
                NodeType::Nan |
                NodeType::Na(_)
        )
    }

    fn is_call(&self) -> bool {
        self.node_type() == NodeType::Call
    }

    fn is_subset(&self) -> bool {
        self.node_type() == NodeType::Subset
    }

    fn is_subset2(&self) -> bool {
        self.node_type() == NodeType::Subset2
    }

    fn is_comment(&self) -> bool {
        self.node_type() == NodeType::Comment
    }

    fn is_braced_expression(&self) -> bool {
        self.node_type() == NodeType::BracedExpression
    }

    fn is_function_definition(&self) -> bool {
        self.node_type() == NodeType::FunctionDefinition
    }

    fn is_if_statement(&self) -> bool {
        self.node_type() == NodeType::IfStatement
    }

    fn is_argument(&self) -> bool {
        self.node_type() == NodeType::Argument
    }

    fn is_arguments(&self) -> bool {
        self.node_type() == NodeType::Arguments
    }

    fn is_namespace_operator(&self) -> bool {
        matches!(self.node_type(), NodeType::NamespaceOperator(_))
    }

    fn is_namespace_internal_operator(&self) -> bool {
        self.node_type() == NodeType::NamespaceOperator(NamespaceOperatorType::Internal)
    }

    fn is_unary_operator(&self) -> bool {
        matches!(self.node_type(), NodeType::UnaryOperator(_))
    }

    fn is_binary_operator(&self) -> bool {
        matches!(self.node_type(), NodeType::BinaryOperator(_))
    }

    fn is_native_pipe_operator(&self) -> bool {
        self.node_type() == NodeType::BinaryOperator(BinaryOperatorType::Pipe)
    }

    fn is_magrittr_pipe_operator(&self, contents: &ropey::Rope) -> anyhow::Result<bool> {
        if self.node_type() != NodeType::BinaryOperator(BinaryOperatorType::Special) {
            return Ok(false);
        }

        let Some(operator) = self.child_by_field_name("operator") else {
            return Ok(false);
        };

        let text = contents.node_slice(&operator)?;

        Ok(text == "%>%")
    }

    fn is_pipe_operator(&self, contents: &ropey::Rope) -> anyhow::Result<bool> {
        if self.is_native_pipe_operator() {
            return Ok(true);
        }

        if self.is_magrittr_pipe_operator(contents)? {
            return Ok(true);
        }

        Ok(false)
    }
}

pub(crate) fn node_text(node: &Node, contents: &ropey::Rope) -> Option<String> {
    contents.node_slice(node).ok().map(|f| f.to_string())
}

pub(crate) fn node_has_error_or_missing(node: &Node) -> bool {
    // According to the docs, `node.has_error()` should return `true`
    // if `node` is itself an error, or if it contains any errors, but that
    // doesn't seem to be the case for terminal ERROR nodes.
    // https://github.com/tree-sitter/tree-sitter/issues/3623
    node.is_error() || node.has_error()
}

pub(crate) fn node_find_string<'a>(node: &'a Node) -> Option<Node<'a>> {
    // If we are on one of the following, we return the string parent:
    // - Anonymous node inside a string, like `"'"`
    // - `NodeType::StringContent`
    // - `NodeType::EscapeSequence`
    // Note that `ancestors()` is actually inclusive, so the original `node`
    // is also considered as a potential string here.
    node.ancestors().find(|node| node.is_string())
}

pub(crate) fn node_in_string(node: &Node) -> bool {
    node_find_string(node).is_some()
}

pub(crate) fn node_is_call(node: &Node, name: &str, contents: &ropey::Rope) -> bool {
    if !node.is_call() {
        return false;
    }
    let Some(fun) = node.child_by_field_name("function") else {
        return false;
    };
    let Some(fun) = node_text(&fun, contents) else {
        return false;
    };
    fun == name
}

pub(crate) fn node_arg_value<'tree>(
    args: &Node<'tree>,
    name: &str,
    contents: &ropey::Rope,
) -> Option<Node<'tree>> {
    if args.node_type() != NodeType::Argument {
        return None;
    }
    let Some(name_node) = args.child_by_field_name("name") else {
        return None;
    };
    let Some(value_node) = args.child_by_field_name("value") else {
        return None;
    };
    let Some(name_text) = node_text(&name_node, contents) else {
        return None;
    };
    (name_text == name).then_some(value_node)
}

pub(crate) fn args_find_call<'tree>(
    args: Node<'tree>,
    name: &str,
    contents: &ropey::Rope,
) -> Option<Node<'tree>> {
    let mut cursor = args.walk();
    let mut iter = args.children(&mut cursor);

    iter.find_map(|n| {
        let value = n.child_by_field_name("value")?;
        node_is_call(&value, name, contents).then_some(value)
    })
}

pub(crate) fn args_find_call_args<'tree>(
    args: Node<'tree>,
    name: &str,
    contents: &ropey::Rope,
) -> Option<Node<'tree>> {
    let call = args_find_call(args, name, contents)?;
    call.child_by_field_name("arguments")
}
