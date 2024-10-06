use tree_sitter::Node;

use crate::lsp;
use crate::treesitter::args_find_call_args;
use crate::treesitter::node_arg_value;
use crate::treesitter::node_is_call;
use crate::treesitter::node_text;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

pub(crate) struct TopLevelDeclare {
    pub(crate) diagnostics: bool,
}

impl Default for TopLevelDeclare {
    fn default() -> Self {
        Self { diagnostics: true }
    }
}

pub(crate) fn top_level_declare(
    ast: &tree_sitter::Tree,
    contents: &ropey::Rope,
) -> TopLevelDeclare {
    let mut decls = TopLevelDeclare::default();

    let Some(declare_args) = top_level_declare_args(ast, contents) else {
        return decls;
    };
    let Some(ark_args) = declare_ark_args(declare_args, contents) else {
        return decls;
    };
    let Some(diagnostics_args) = ark_diagnostics_args(ark_args, contents) else {
        return decls;
    };

    let mut cursor = diagnostics_args.walk();
    let mut iter = diagnostics_args.children(&mut cursor);

    let Some(enable) = iter.find_map(|n| node_arg_value(&n, "enable", contents)) else {
        return decls;
    };
    let Some(enable_text) = node_text(&enable, contents) else {
        return decls;
    };

    if enable_text == "FALSE" {
        decls.diagnostics = false;
    } else if enable_text != "TRUE" {
        lsp::log_warn!("Invalid `diagnostics = ` declaration");
    }

    decls
}

fn top_level_declare_args<'tree>(
    ast: &'tree tree_sitter::Tree,
    contents: &ropey::Rope,
) -> Option<Node<'tree>> {
    let root = ast.root_node();
    let mut cursor = root.walk();
    let iter = root.children(&mut cursor);

    // The declarations are allowed to appear after top comments
    let mut iter = iter.skip_while(|n| n.is_comment());
    let Some(mut first) = iter.next() else {
        return None;
    };

    // For backward compatibility with R < 4.4.0, declarations may be wrapped in
    // a tilde call
    if first.node_type() == NodeType::UnaryOperator(UnaryOperatorType::Tilde) {
        first = first.child_by_field_name("rhs")?;
    }

    if !node_is_call(&first, "declare", contents) {
        return None;
    }

    first.child_by_field_name("arguments")
}

fn declare_ark_args<'tree>(
    declare_args: Node<'tree>,
    contents: &ropey::Rope,
) -> Option<Node<'tree>> {
    args_find_call_args(declare_args, "ark", contents)
}

fn ark_diagnostics_args<'tree>(
    ark_args: Node<'tree>,
    contents: &ropey::Rope,
) -> Option<Node<'tree>> {
    args_find_call_args(ark_args, "diagnostics", contents)
}

#[cfg(test)]
mod test {
    use stdext::assert_match;

    use crate::lsp::declarations::declare_ark_args;
    use crate::lsp::declarations::top_level_declare;
    use crate::lsp::declarations::top_level_declare_args;
    use crate::lsp::documents::Document;

    #[test]
    fn test_declare_args() {
        let doc = Document::new("", None);
        assert_match!(top_level_declare_args(&doc.ast, &doc.contents), None);

        let doc = Document::new("declare()", None);
        assert_match!(top_level_declare_args(&doc.ast, &doc.contents), Some(_));

        let doc = Document::new("~declare()", None);
        assert_match!(top_level_declare_args(&doc.ast, &doc.contents), Some(_));

        let doc = Document::new("# foo\n#bar\n\ndeclare()", None);
        assert_match!(top_level_declare_args(&doc.ast, &doc.contents), Some(_));

        let doc = Document::new("# foo\nbar\n\ndeclare()", None);
        assert_match!(top_level_declare_args(&doc.ast, &doc.contents), None);
    }

    #[test]
    fn test_declare_ark_args() {
        let doc = Document::new("declare()", None);
        let decls = top_level_declare_args(&doc.ast, &doc.contents).unwrap();
        assert_match!(declare_ark_args(decls, &doc.contents), None);

        let doc = Document::new("declare(ark())", None);
        let decls = top_level_declare_args(&doc.ast, &doc.contents).unwrap();
        assert_match!(declare_ark_args(decls, &doc.contents), Some(_));

        let doc = Document::new("declare(foo, ark())", None);
        let decls = top_level_declare_args(&doc.ast, &doc.contents).unwrap();
        assert_match!(declare_ark_args(decls, &doc.contents), Some(_));
    }

    #[test]
    fn test_declare_diagnostics() {
        let doc = Document::new("", None);
        let decls = top_level_declare(&doc.ast, &doc.contents);
        assert_eq!(decls.diagnostics, true);

        let doc = Document::new("declare(ark(diagnostics(enable = TRUE)))", None);
        let decls = top_level_declare(&doc.ast, &doc.contents);
        assert_eq!(decls.diagnostics, true);

        let doc = Document::new("declare(ark(diagnostics(enable = NULL)))", None);
        let decls = top_level_declare(&doc.ast, &doc.contents);
        assert_eq!(decls.diagnostics, true);

        let doc = Document::new("declare(ark(diagnostics(enable = invalid())))", None);
        let decls = top_level_declare(&doc.ast, &doc.contents);
        assert_eq!(decls.diagnostics, true);

        let doc = Document::new("~declare()", None);
        let decls = top_level_declare(&doc.ast, &doc.contents);
        assert_eq!(decls.diagnostics, true);

        let doc = Document::new("declare(ark(diagnostics(enable = FALSE)))", None);
        let decls = top_level_declare(&doc.ast, &doc.contents);
        assert_eq!(decls.diagnostics, false);
    }
}
