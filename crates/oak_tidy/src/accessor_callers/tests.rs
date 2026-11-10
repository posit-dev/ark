use super::render_from_sources;

fn report(files: &[(&str, &str)]) -> String {
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(rel_path, source)| (rel_path.to_string(), source.to_string()))
        .collect();
    render_from_sources(&owned)
}

const EMPTY_REPORT: &str = "== broad accessor callers ==";

#[test]
fn lists_method_call() {
    let source = "\
fn recognize(db: &dyn Db, file: File) {
    let root = file.syntax_tree(db);
}
";
    let expected = "\
== broad accessor callers ==
ide/src/foo.rs: recognize -> syntax_tree";
    assert_eq!(report(&[("ide/src/foo.rs", source)]), expected);
}

#[test]
fn lists_path_call() {
    let source = "\
fn recognize(db: &dyn Db, file: File) {
    let root = File::syntax_tree(file, db);
}
";
    let expected = "\
== broad accessor callers ==
ide/src/foo.rs: recognize -> syntax_tree";
    assert_eq!(report(&[("ide/src/foo.rs", source)]), expected);
}

#[test]
fn lists_function_reference() {
    let source = "\
fn recognize(db: &dyn Db, files: Vec<File>) {
    let roots = files.into_iter().map(File::syntax_tree);
    let roots = files.into_iter().map(<File>::syntax_tree);
}
";
    let expected = "\
== broad accessor callers ==
ide/src/foo.rs: recognize -> syntax_tree";
    assert_eq!(report(&[("ide/src/foo.rs", source)]), expected);
}

#[test]
fn lists_calls_inside_macros() {
    let source = "\
fn method_in_macro(db: &dyn Db, file: File) {
    log::trace!(\"{:?}\", file.syntax_tree(db));
}

fn path_in_macro(db: &dyn Db, file: File) {
    assert_eq!(File::syntax_tree(file, db), other);
}

fn field_in_macro(holder: Holder) {
    log::trace!(\"{:?}\", holder.syntax_tree);
}
";
    let expected = "\
== broad accessor callers ==
ide/src/foo.rs: method_in_macro -> syntax_tree
ide/src/foo.rs: path_in_macro -> syntax_tree";
    assert_eq!(report(&[("ide/src/foo.rs", source)]), expected);
}

#[test]
fn does_not_list_calls_inside_owning_crate() {
    let source = "\
fn classify(db: &dyn Db, file: File) {
    let root = file.syntax_tree(db);
}
";
    assert_eq!(
        report(&[("oak_db/src/identifier.rs", source)]),
        EMPTY_REPORT
    );
}

#[test]
fn does_not_list_same_name_non_calls_or_other_names() {
    let source = "\
fn other_names(db: &dyn Db, file: File, holder: Holder) {
    let number = \"1\".parse::<u32>();
    let parsed = file.parse(db);
    let tree = holder.syntax_tree;
    let syntax_tree = 1;
    let copy = syntax_tree;
    let text = file.syntax_tree_text(db);
    let built = syntax_tree(db);
}
";
    assert_eq!(report(&[("ide/src/foo.rs", source)]), EMPTY_REPORT);
}

#[test]
fn names_enclosing_method_and_closure() {
    let source = "\
impl<'db> Recognizer<'db> {
    fn context(&self, db: &dyn Db) {
        let root = self.file.syntax_tree(db);
    }
}

impl Handler for Server {
    fn handle(&self, db: &dyn Db) {
        self.files.iter().for_each(|file| {
            let root = file.syntax_tree(db);
        });
    }
}

fn outer(db: &dyn Db, file: File) {
    fn inner(db: &dyn Db, file: File) {
        let root = file.syntax_tree(db);
    }
}

mod nested {
    fn free(db: &dyn Db, file: File) {
        let root = file.syntax_tree(db);
    }
}
";
    let expected = "\
== broad accessor callers ==
ide/src/foo.rs: Recognizer::context -> syntax_tree
ide/src/foo.rs: Server::handle -> syntax_tree
ide/src/foo.rs: nested::free -> syntax_tree
ide/src/foo.rs: outer::inner -> syntax_tree";
    assert_eq!(report(&[("ide/src/foo.rs", source)]), expected);
}

#[test]
fn deduplicates_calls_within_one_function() {
    let source = "\
fn twice(db: &dyn Db, file: File) {
    let first = file.syntax_tree(db);
    let second = File::syntax_tree(file, db);
}
";
    let expected = "\
== broad accessor callers ==
ide/src/foo.rs: twice -> syntax_tree";
    assert_eq!(report(&[("ide/src/foo.rs", source)]), expected);
}

#[test]
fn skips_test_code() {
    let source = "\
#[cfg(test)]
mod tests {
    fn helper(db: &dyn Db, file: File) {
        let root = file.syntax_tree(db);
    }
}

#[test]
fn loose_test() {
    let root = file.syntax_tree(db);
}
";
    let test_file = "\
fn helper(db: &dyn Db, file: File) {
    let root = file.syntax_tree(db);
}
";
    assert_eq!(
        report(&[
            ("ide/src/foo.rs", source),
            ("ide/src/tests/support.rs", test_file),
            ("ide/src/foo/tests.rs", test_file),
        ]),
        EMPTY_REPORT
    );
}

#[test]
fn sorts_across_files() {
    let source = "\
fn caller(db: &dyn Db, file: File) {
    let root = file.syntax_tree(db);
}
";
    let expected = "\
== broad accessor callers ==
a/src/lib.rs: caller -> syntax_tree
b/src/lib.rs: caller -> syntax_tree";
    assert_eq!(
        report(&[("b/src/lib.rs", source), ("a/src/lib.rs", source)]),
        expected
    );
}
