//
// string.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;

use aether_path::FilePath;
use anyhow::anyhow;
use anyhow::Context;
use biome_line_index::LineCol;
use legacy_ide::traits::node::NodeExt;
use legacy_ide::treesitter::node_find_string;
use oak_core::string::decode_string_prefix;
use oak_db::Db;
use oak_db::File;
use oak_ide::file_path_completions;
use stdext::result::ResultExt;
use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_file_path;
use crate::lsp::completions::request::UniqueRequest;
use crate::lsp::completions::sources::unique::subset::node_find_object_for_string_subset;
use crate::lsp::completions::sources::unique::Recognized;
use crate::lsp::completions::sources::unique::UniqueCompletions;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;

/// Recognizes a cursor inside a string, suppressing later sources even when
/// there are no items.
pub(super) fn recognize_string(
    completion_context: &CompletionContext,
) -> anyhow::Result<Option<Recognized>> {
    let context = completion_context.document_context;
    let node = context.node;

    // Find actual `NodeType::String` node. Needed in case we are in its children.
    let Some(node) = node_find_string(&node) else {
        return Ok(None);
    };

    // At `|""|`, the cursor is outside the string. At the end of an
    // unclosed string, as in `"dat|`, it is inside because the missing closing
    // quote has no width.
    let closed = node
        .child_by_field_name("close")
        .is_some_and(|close| !close.is_missing());
    if node.start_position() == context.point || (closed && node.end_position() == context.point) {
        return Ok(None);
    }

    // Return empty set if we are here due to a trigger character like `$`.
    // See posit-dev/positron#1884.
    if context.trigger.is_some() {
        return Ok(Some(Recognized::Empty));
    }

    // Check if we are doing string subsetting, like `x["<tab>"]`. This is a very unique
    // case that takes priority over file path completions.
    if let Some(object) = node_find_object_for_string_subset(&node, context) {
        let object = object.node_as_str(context.contents)?.to_string();
        return Ok(Some(Recognized::Session(UniqueRequest::StringSubset {
            object,
        })));
    }

    // If no special string cases are hit, we show file path completions
    let cursor = context
        .line_index
        .offset(LineCol {
            line: context.point.row as u32,
            col: context.point.column as u32,
        })
        .ok_or_else(|| anyhow!("Can't find the offset of {}", context.point))?;
    let prefix = context
        .contents
        .get(node.start_byte()..usize::from(cursor))
        .ok_or_else(|| anyhow!("Can't slice the string before {}", context.point))?;
    Ok(Some(Recognized::Static(UniqueCompletions::new(
        file_path_items(completion_context, prefix)?,
    ))))
}

/// Log directory-listing failures and return no items rather than failing
/// the completion request.
///
/// `prefix` is the string literal's text from its opening quote up to the
/// cursor. Text after the cursor must not affect which directory is listed.
fn file_path_items(
    completion_context: &CompletionContext,
    prefix: &str,
) -> anyhow::Result<Vec<CompletionItem>> {
    let Some(value) = decode_string_prefix(prefix) else {
        return Ok(Vec::new());
    };

    let db = completion_context.state.db();
    let file = completion_context.file;
    let fallback_dir = fallback_dir(db, file);

    let Some(entries) = file_path_completions(db, file, &value, fallback_dir.as_deref())
        .with_context(|| format!("Can't list file path completions for {prefix}"))
        .log_err()
    else {
        return Ok(Vec::new());
    };

    let mut items = entries
        .into_iter()
        .map(completion_item_from_file_path)
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Rank names starting with punctuation or `_` after word-leading names.
    set_sort_text_by_words_first(&mut items);

    Ok(items)
}

/// The directory a relative path resolves against in a file without an anchor
/// directory. Console input uses the process working directory, which follows
/// `setwd()` because R runs in this process. Other such files, like untitled
/// documents, use the first workspace root.
fn fallback_dir(db: &dyn Db, file: File) -> Option<PathBuf> {
    if is_console_input(file.path(db)) {
        return std::env::current_dir().log_err();
    }

    let roots = db.workspace_roots().roots(db);
    let root = roots.first()?;
    Some(root.path(db).as_path()?.as_std_path().to_path_buf())
}

/// Positron's console sends its input as `inmemory:` documents.
fn is_console_input(path: &FilePath) -> bool {
    matches!(path, FilePath::Virtual(uri) if uri.as_url().scheme() == "inmemory")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use aether_path::FilePath;
    use harp::exec::RFunction;
    use harp::exec::RFunctionExt;
    use harp::utils::r_is_string;
    use legacy_ide::fixtures::point_from_cursor;
    use legacy_ide::treesitter::node_find_string;
    use legacy_ide::treesitter::NodeTypeExt;
    use oak_core::string::decode_string;
    use oak_db::DbInputs;
    use oak_db::File;
    use oak_db::Root;
    use oak_db::RootKind;
    use oak_lsp::state::WorldState;
    use oak_scan::DbScan;
    use salsa::Setter;
    use tower_lsp_server::ls_types::CompletionItemKind;
    use tree_sitter::Point;

    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::finish;
    use crate::lsp::completions::request::RCompletionReplies;
    use crate::lsp::completions::request::RCompletionRequest;
    use crate::lsp::completions::sources::unique;
    use crate::lsp::completions::sources::unique::string::recognize_string;
    use crate::lsp::completions::sources::unique::Recognized;
    use crate::lsp::completions::sources::unique::UniqueCompletions;
    use crate::lsp::completions::static_completions;
    use crate::lsp::document_context::test_document_context;
    use crate::lsp::document_context::TestDocument;
    use crate::panic;
    use crate::r_task;

    fn recognize_in(
        doc: &TestDocument,
        point: Point,
        trigger: Option<String>,
    ) -> Option<Recognized> {
        let _boundary = panic::test_boundary();
        let context = doc.context_with_trigger(point, trigger);
        let completion_context = CompletionContext::new(&context, doc.file(), doc.state());
        recognize_string(&completion_context).unwrap()
    }

    /// Opens `text` as `script.R` in `dir`, outside any workspace root, so
    /// relative paths resolve against `dir`.
    fn open_in_dir(state: &mut WorldState, dir: &Path, text: &str) -> File {
        let _boundary = panic::test_boundary();
        let path = FilePath::from_path_buf(dir.join("script.R")).unwrap();
        state.db_mut().upsert_editor(path, text.to_string())
    }

    fn write_file(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn test_outside_quotes() {
        // Before or after the `''`, i.e. `|''` or `''|`.
        // Still considered part of the string node.
        let (text, point) = point_from_cursor("@''");
        let doc = TestDocument::new(&text);

        assert!(node_find_string(&doc.context(point).node).is_some());
        assert_eq!(recognize_in(&doc, point, None), None);
    }

    #[test]
    fn test_not_string() {
        let (text, point) = point_from_cursor("@foo");
        let doc = TestDocument::new(&text);

        assert!(doc.context(point).node.is_identifier());
        assert_eq!(recognize_in(&doc, point, None), None);
    }

    #[test]
    fn test_trigger() {
        let _boundary = panic::test_boundary();
        let (text, point) = point_from_cursor("'~/@'");
        let doc = TestDocument::new(&text);

        assert!(matches!(
            recognize_in(&doc, point, None),
            Some(Recognized::Static(UniqueCompletions {
                incomplete_until_packages_settle,
                ..
            })) if incomplete_until_packages_settle.is_empty()
        ));

        // `Some` trigger -> Should return empty completion set
        let trigger = Some(String::from("$"));
        assert_eq!(
            recognize_in(&doc, point, trigger.clone()),
            Some(Recognized::Empty)
        );

        // Check for same result when consulting (potentially all) unique sources
        let context = doc.context_with_trigger(point, trigger);
        let completion_context = CompletionContext::new(&context, doc.file(), doc.state());
        let mut request = RCompletionRequest::default();
        let res = unique::get_completions(&completion_context, &mut request).unwrap();

        let res = res.unwrap();
        assert_eq!(res.items, vec![]);
        assert_eq!(res.incomplete_until_packages_settle, vec![]);
        assert_eq!(request, RCompletionRequest::default());
    }

    #[test]
    fn test_file_paths_need_no_session() {
        let _boundary = panic::test_boundary();
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("R/utils.R"));
        std::fs::create_dir_all(dir.path().join("R/sub")).unwrap();

        let mut state = WorldState::default();
        let (text, point) = point_from_cursor("source('R/@')");
        let file = open_in_dir(&mut state, dir.path(), &text);
        let context = test_document_context(&state, file, point, None);

        let (completions, request) = static_completions(&context, file, &state).unwrap();
        assert!(!request.has_session_slots());

        let items = finish(completions, RCompletionReplies::default());
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, vec!["sub/", "utils.R"]);
        assert_eq!(items[0].kind, Some(CompletionItemKind::FOLDER));
        assert_eq!(items[1].kind, Some(CompletionItemKind::FILE));
    }

    fn file_path_labels(state: &WorldState, file: File, point: Point) -> Vec<String> {
        let _boundary = panic::test_boundary();
        let context = test_document_context(state, file, point, None);
        let (completions, _) = static_completions(&context, file, state).unwrap();
        finish(completions, RCompletionReplies::default())
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    #[test]
    fn test_file_paths_in_unclosed_string_at_end_of_file() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("data.csv"));
        write_file(&dir.path().join("R/utils.R"));

        let mut state = WorldState::default();
        let (text, point) = point_from_cursor("x <- 1\n\"dat@");
        let file = open_in_dir(&mut state, dir.path(), &text);
        assert_eq!(file_path_labels(&state, file, point), vec![
            "R/", "data.csv"
        ]);
    }

    #[test]
    fn test_file_paths_ignore_text_after_cursor() {
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("R/utils.R"));
        write_file(&dir.path().join("R/sub/inner.R"));

        let mut state = WorldState::default();
        let (text, point) = point_from_cursor("source('R/@sub/')");
        let file = open_in_dir(&mut state, dir.path(), &text);
        assert_eq!(file_path_labels(&state, file, point), vec![
            "sub/", "utils.R"
        ]);
    }

    #[test]
    fn test_console_input_resolves_against_working_directory() {
        let _boundary = panic::test_boundary();
        // Nextest isolates the working directory change in a separate process.
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("R/utils.R"));
        std::env::set_current_dir(dir.path()).unwrap();

        let mut state = WorldState::default();
        let (text, point) = point_from_cursor("source('R/@')");
        let path = FilePath::parse("inmemory:/repl-r-1").unwrap();
        let file = state.db_mut().upsert_editor(path, text);

        assert_eq!(file_path_labels(&state, file, point), vec!["utils.R"]);
    }

    #[test]
    fn test_untitled_document_resolves_against_first_workspace_root() {
        let _boundary = panic::test_boundary();
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("R/utils.R"));

        let mut state = WorldState::default();
        let root = Root::new(
            state.db(),
            FilePath::from_path_buf(dir.path().to_path_buf()).unwrap(),
            RootKind::Workspace,
            vec![],
            vec![],
            vec![],
        );
        state
            .db()
            .workspace_roots()
            .set_roots(state.db_mut())
            .to(vec![root]);

        let (text, point) = point_from_cursor("source('R/@')");
        let path = FilePath::parse("untitled:Untitled-1").unwrap();
        let file = state.db_mut().upsert_editor(path, text);
        assert_eq!(file_path_labels(&state, file, point), vec!["utils.R"]);

        // Without a workspace, relative paths list nothing
        let mut state = WorldState::default();
        let path = FilePath::parse("untitled:Untitled-1").unwrap();
        let (text, point) = point_from_cursor("source('R/@')");
        let file = state.db_mut().upsert_editor(path, text);
        assert_eq!(file_path_labels(&state, file, point), Vec::<String>::new());
    }

    #[test]
    fn test_file_paths_with_invalid_escape() {
        // https://github.com/posit-dev/positron/issues/6584
        let _boundary = panic::test_boundary();
        let dir = tempfile::tempdir().unwrap();
        write_file(&dir.path().join("R/utils.R"));

        let mut state = WorldState::default();
        let (text, point) = point_from_cursor(r#"".\R\@""#);
        let file = open_in_dir(&mut state, dir.path(), &text);
        let context = test_document_context(&state, file, point, None);

        let (completions, request) = static_completions(&context, file, &state).unwrap();
        assert!(!request.has_session_slots());
        assert_eq!(finish(completions, RCompletionReplies::default()), vec![]);
    }

    /// String literals with escapes R accepts and rejects. `decode_string()`
    /// must agree with R's parser on each. `harp::parse_expr()` converts
    /// `\r\n` to `\n` before parsing, as `readLines()` does for R files.
    const STRING_LITERALS: &[&str] = &[
        r#""abc""#,
        "'abc'",
        r#""""#,
        r#"'a"b'"#,
        "\"\u{e9}t\u{e9}\"",
        "\"a\nb\"",
        r#""a\nb""#,
        r#""\a\b\f\n\r\t\v""#,
        r#""\\""#,
        r#""\"""#,
        r#"'\''"#,
        r#""\'\"\`""#,
        r#""\ ""#,
        "\"a\\\nb\"",
        "\"a\r\nb\"",
        "\"a\\\r\nb\"",
        "\"a\rb\"",
        "\"a\\\rb\"",
        "r\"(a\r\nb)\"",
        r#""C:\\Users\\x""#,
        r#"".\R\utils.R""#,
        r#""\q""#,
        r#""\8""#,
        r#""\101""#,
        r#""\12a""#,
        r#""\1234""#,
        r#""\378""#,
        r#""\0""#,
        r#""\00""#,
        r#""\377""#,
        r#""\400""#,
        r#""\777""#,
        r#""\x41""#,
        r#""\x41g""#,
        r#""\x4142""#,
        r#""\xa""#,
        r#""\xg""#,
        r#""\x""#,
        r#""\x0""#,
        r#""\xff""#,
        r#""\xc3\xa9""#,
        "\"\u{e9}\\xe9\"",
        r#""\u41""#,
        r#""\u00e9""#,
        r#""\u12345""#,
        r#""\u{41}""#,
        r#""\u{0041}""#,
        r#""\u{12}3""#,
        r#""\u{00041}""#,
        r#""\u{1F600}""#,
        r#""\u{41""#,
        r#""\u{}""#,
        r#""\u""#,
        r#""\u{0}""#,
        r#""\u000""#,
        r#""\U1F600""#,
        r#""\U{1F600}""#,
        r#""\U{0001F600}""#,
        r#""\U0010FFFF""#,
        r#""\U{41}""#,
        r#""\U{00001F600}""#,
        r#""\U110000""#,
        r#""\U{110000}""#,
        r#""\UFFFFFFFF""#,
        r#""\U0""#,
        r#""\uD83D\uDE00""#,
        r#""\uD83D\U0000DE00""#,
        r#""\U{D83D}\U{DE00}""#,
        r#""\uD800""#,
        r#""\uDC00x""#,
        r#""\uD83Dx\uDE00""#,
        r#""\uDE00\uD83D""#,
        r#""\x41\u0041""#,
        r#""\u00e9\x41""#,
        r#""\101\u0041""#,
        "\"\u{e9}\\u00e9\"",
        "\"\u{e9}\\x41\"",
        r#"r"(a\b)""#,
        r#"R"[x]""#,
        r#"r"{x}""#,
        "r'(x)'",
        r#"r"()""#,
        r#"r"(a)b)""#,
        r#"r"---(a)"--)---""#,
        r#"r"-(a)-)-""#,
        "r\"(a\nb)\"",
        r#"r"(\R\utils.R)""#,
        r#"r"(x]""#,
        r#"r"-(x)""#,
        r#"r"-(a)--"-""#,
        r#"r"""#,
        r#"r"x""#,
        r#""abc"#,
        r#""a\""#,
    ];

    /// The value R's parser gives `literal`, or `None` if it rejects the
    /// literal or the value isn't valid UTF-8.
    fn r_string_value(literal: &str) -> Option<String> {
        let value = harp::parse_expr(literal).ok()?;
        if !r_is_string(value.sexp) {
            return None;
        }

        let valid = RFunction::new("base", "validUTF8")
            .add(value.clone())
            .call()
            .unwrap();
        if !bool::try_from(valid).unwrap() {
            return None;
        }

        Some(String::try_from(value).unwrap())
    }

    #[test]
    fn test_decode_string_agrees_with_r() {
        r_task(|| {
            for literal in STRING_LITERALS {
                assert_eq!(
                    (literal, decode_string(literal)),
                    (literal, r_string_value(literal))
                );
            }
        })
    }
}
