use tree_sitter::Node;

#[derive(Debug, PartialEq)]
pub enum NodeType {
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
    UnmatchedDelimiter(UnmatchedDelimiterType),
    Error,
    Anonymous(String),
}

fn node_type(x: &Node) -> NodeType {
    match x.kind() {
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
        "unmatched_delimiter" => NodeType::UnmatchedDelimiter(unmatched_delimiter_type(x)),
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
        _ => std::unreachable!(),
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
        _ => std::unreachable!(),
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
        _ => std::unreachable!(),
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
        _ => std::unreachable!(),
    }
}

#[derive(Debug, PartialEq)]
pub enum UnmatchedDelimiterType {
    /// `}`
    Brace,
    /// `)`
    Parenthesis,
    /// `]`
    Bracket,
}

fn unmatched_delimiter_type(x: &Node) -> UnmatchedDelimiterType {
    let x = x.child(0).unwrap();

    match x.kind() {
        "}" => UnmatchedDelimiterType::Brace,
        ")" => UnmatchedDelimiterType::Parenthesis,
        "]" => UnmatchedDelimiterType::Bracket,
        _ => std::unreachable!(),
    }
}

pub trait NodeTypeExt: Sized {
    fn node_type(&self) -> NodeType;

    fn is_identifier(&self) -> bool;
    fn is_string(&self) -> bool;
    fn is_identifier_or_string(&self) -> bool;
    fn is_keyword(&self) -> bool;
    fn is_call(&self) -> bool;
    fn is_comment(&self) -> bool;
    fn is_braced_expression(&self) -> bool;
    fn is_function_definition(&self) -> bool;
    fn is_if_statement(&self) -> bool;
    fn is_namespace_operator(&self) -> bool;
    fn is_namespace_internal_operator(&self) -> bool;
    fn is_unary_operator(&self) -> bool;
    fn is_binary_operator(&self) -> bool;
}

impl NodeTypeExt for Node<'_> {
    fn node_type(&self) -> NodeType {
        node_type(self)
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
}