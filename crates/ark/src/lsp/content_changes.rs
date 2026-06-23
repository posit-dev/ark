use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::PositionEncoding;
use tower_lsp::lsp_types;

// --- source
// authors = ["rust-analyzer team"]
// license = "MIT OR Apache-2.0"
// origin = "https://github.com/rust-lang/rust-analyzer/blob/master/crates/rust-analyzer/src/lsp/utils.rs"
// ---
/// Apply a batch of LSP content changes to `contents`, returning the new text.
pub(crate) fn apply_content_changes(
    contents: &str,
    content_changes: &[lsp_types::TextDocumentContentChangeEvent],
    encoding: PositionEncoding,
) -> String {
    let mut contents = contents.to_string();
    let mut changes = content_changes.to_vec();

    // If at least one of the changes is a full document change, use the last of them
    // as the starting point and ignore all previous changes. We then know that all
    // changes after this (if any!) are incremental changes.
    //
    // If we do have a full document change, that implies the `last_start_line`
    // corresponding to that change is line 0, which will correctly force a rebuild
    // of the line index before applying any incremental changes.
    let (changes, mut last_start_line) =
        match changes.iter().rposition(|change| change.range.is_none()) {
            Some(idx) => {
                let incremental = changes.split_off(idx + 1);
                // Unwrap: `rposition()` confirmed this index contains a full document change
                let change = changes.pop().unwrap();
                contents = change.text;
                (incremental, 0)
            },
            None => (changes, u32::MAX),
        };

    let mut line_index = biome_line_index::LineIndex::new(&contents);

    // Handle all incremental changes after the last full document change. We don't
    // typically get >1 incremental change as the user types, but we do get them in a
    // batch after a find-and-replace, or after a format-on-save request.
    //
    // Some editors like VS Code send the edits in reverse order (from the bottom of
    // file -> top of file). We can take advantage of this, because applying an edit
    // on, say, line 10, doesn't invalidate the `line_index` if we then need to apply
    // an additional edit on line 5. That said, we may still have edits that cross
    // lines, so rebuilding the `line_index` is not always unavoidable.
    for change in changes {
        let range = change
            .range
            .expect("`None` case already handled by finding the last full document change.");

        // If the end of this change is at or past the start of the last change, then
        // the `line_index` needed to apply this change is now invalid, so we have to
        // rebuild it.
        if range.end.line >= last_start_line {
            line_index = biome_line_index::LineIndex::new(&contents);
        }
        last_start_line = range.start.line;

        // This is a panic if we can't convert. It means we can't keep the document up
        // to date and something is very wrong.
        let range: std::ops::Range<usize> = from_proto::text_range(range, &line_index, encoding)
            .expect("Can convert `range` from `Position` to `TextRange`.")
            .into();

        contents.replace_range(range, &change.text);
    }

    contents
}

#[cfg(test)]
mod tests {
    use biome_line_index::WideEncoding;

    use super::*;

    const ENCODING: PositionEncoding = PositionEncoding::Wide(WideEncoding::Utf16);

    fn insert(text: &str, line: u32, character: u32) -> lsp_types::TextDocumentContentChangeEvent {
        let position = lsp_types::Position::new(line, character);
        lsp_types::TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range::new(position, position)),
            range_length: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn test_apply_content_changes_incremental_inserts() {
        // Type "lib" one character at a time, the way an editor streams it.
        let after_l = apply_content_changes("", &[insert("l", 0, 0)], ENCODING);
        assert_eq!(after_l, "l");

        let after_i = apply_content_changes(&after_l, &[insert("i", 0, 1)], ENCODING);
        assert_eq!(after_i, "li");

        let after_b = apply_content_changes(&after_i, &[insert("b", 0, 2)], ENCODING);
        assert_eq!(after_b, "lib");
    }

    #[test]
    fn test_apply_content_changes_full_replacement_wins() {
        // A range-less change replaces the whole buffer; earlier changes in the
        // batch are discarded, later incremental ones apply on top of it.
        let changes = vec![
            insert("ignored", 0, 0),
            lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "abc\n".to_string(),
            },
            insert("X", 0, 3),
        ];
        assert_eq!(apply_content_changes("old", &changes, ENCODING), "abcX\n");
    }
}
