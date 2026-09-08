use super::render_from_sources;

fn report(files: &[(&str, &str)]) -> String {
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(rel_path, source)| (rel_path.to_string(), source.to_string()))
        .collect();
    render_from_sources(&owned)
}

const EMPTY_REPORT: &str = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==

== cycle recovery ==";

#[test]
fn renders_free_query() {
    let source = "\
#[salsa::tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn foo(db: &dyn Db) -> u32

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_tracked_impl_method() {
    let source = "\
impl Foo {
    #[salsa::tracked]
    fn bar(self, db: &dyn Db) -> u32 {
        0
    }
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn Foo::bar(self, db: &dyn Db) -> u32

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_input_struct() {
    let source = "\
#[salsa::input]
pub struct Foo {
    bar: u32,
}
";
    let expected = "\
== inputs ==
foo.rs: struct Foo
  bar: u32

== interned ==

== tracked structs ==

== tracked queries ==

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_interned_struct() {
    let source = "\
#[salsa::interned(debug)]
pub struct Name<'db> {
    text: String,
}
";
    let expected = "\
== inputs ==

== interned ==
foo.rs: struct Name<'db> [debug]
  text: String

== tracked structs ==

== tracked queries ==

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_tracked_struct_with_field_options() {
    let source = "\
#[salsa::tracked(debug)]
pub struct Definition<'db> {
    #[tracked]
    kind: DefinitionKind,
    #[no_eq]
    revision: FileRevision,
    #[returns(copy)]
    plain: u32,
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==
foo.rs: struct Definition<'db> [debug]
  kind: DefinitionKind [tracked]
  revision: FileRevision [no_eq]
  plain: u32 [returns(copy)]

== tracked queries ==

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_query_option_returns_ref() {
    let source = "\
#[salsa::tracked(returns(ref))]
fn foo(db: &dyn Db) -> Vec<u32> {
    Vec::new()
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn foo(db: &dyn Db) -> Vec<u32> [returns(ref)]

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_query_option_no_eq() {
    let source = "\
#[salsa::tracked(no_eq)]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn foo(db: &dyn Db) -> u32 [no_eq]

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_query_option_lru() {
    let source = "\
#[salsa::tracked(lru = 128)]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn foo(db: &dyn Db) -> u32 [lru = 128]

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_bare_tracked_query_with_no_options() {
    let source = "\
#[salsa::tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn foo(db: &dyn Db) -> u32

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_single_cycle_result_unchanged() {
    let source = "\
#[salsa::tracked(returns(ref), cycle_result = foo_cycle_result)]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn foo(db: &dyn Db) -> u32 [returns(ref), cycle_result = foo_cycle_result]

== cycle recovery ==
foo.rs: foo -> foo_cycle_result";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn excludes_file_named_tests_rs() {
    let source = "\
#[salsa::tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    assert_eq!(report(&[("tests.rs", source)]), EMPTY_REPORT);
}

#[test]
fn excludes_files_under_tests_directory() {
    let source = "\
#[salsa::tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    assert_eq!(report(&[("tests/foo.rs", source)]), EMPTY_REPORT);
}

#[test]
fn excludes_inline_cfg_test_mod() {
    let source = "\
#[cfg(test)]
mod tests {
    #[salsa::tracked]
    fn foo(db: &dyn Db) -> u32 {
        0
    }
}
";
    assert_eq!(report(&[("foo.rs", source)]), EMPTY_REPORT);
}

#[test]
fn excludes_item_level_cfg_test_on_tracked_fn() {
    let source = "\
#[cfg(test)]
#[salsa::tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    assert_eq!(report(&[("foo.rs", source)]), EMPTY_REPORT);
}

#[test]
fn renders_nested_tracked_fn_in_free_fn_body() {
    let source = "\
fn outer(db: &dyn Db) -> u32 {
    #[salsa::tracked]
    fn inner(db: &dyn Db) -> u32 {
        0
    }
    inner(db)
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn inner(db: &dyn Db) -> u32

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_nested_tracked_fn_in_impl_method_body() {
    let source = "\
impl Foo {
    fn outer(self, db: &dyn Db) -> u32 {
        #[salsa::tracked]
        fn inner(db: &dyn Db) -> u32 {
            0
        }
        inner(db)
    }
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn Foo::inner(db: &dyn Db) -> u32

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn renders_fixpoint_and_cycle_result_side_by_side() {
    let source = "\
#[salsa::tracked(returns(ref), cycle_fn = bar_recover, cycle_initial = bar_initial)]
fn bar(db: &dyn Db) -> u32 {
    0
}

#[salsa::tracked(cycle_result = baz_cycle_result)]
fn baz(db: &dyn Db) -> u32 {
    0
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn bar(db: &dyn Db) -> u32 [returns(ref), cycle_fn = bar_recover, cycle_initial = bar_initial]
foo.rs: fn baz(db: &dyn Db) -> u32 [cycle_result = baz_cycle_result]

== cycle recovery ==
foo.rs: bar -> cycle_fn = bar_recover, cycle_initial = bar_initial
foo.rs: baz -> baz_cycle_result";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn recognizes_salsa_macros_tracked() {
    let source = "\
#[salsa_macros::tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==

== tracked queries ==
foo.rs: fn foo(db: &dyn Db) -> u32

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
#[should_panic]
fn bare_tracked_on_item_panics() {
    let source = "\
#[tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
";
    report(&[("foo.rs", source)]);
}

/// The reconciliation is the net for declaration forms nobody enumerated, so it
/// needs a case proving it fires. A `macro_rules!` body is also the documented
/// unsupported case: the text scan counts the attribute, the walk cannot reach
/// it, and the scanner refuses rather than under-reporting.
#[test]
#[should_panic(expected = "a salsa declaration form is present that the walk does not handle")]
fn unreachable_attribute_fails_reconciliation() {
    let source = "\
macro_rules! define_query {
    () => {
        #[salsa::tracked]
        fn foo(db: &dyn Db) -> u32 {
            0
        }
    };
}
";
    report(&[("foo.rs", source)]);
}

#[test]
fn bare_tracked_on_struct_field_does_not_panic() {
    let source = "\
#[salsa::tracked]
pub struct Foo<'db> {
    #[tracked]
    bar: u32,
}
";
    let expected = "\
== inputs ==

== interned ==

== tracked structs ==
foo.rs: struct Foo<'db>
  bar: u32 [tracked]

== tracked queries ==

== cycle recovery ==";
    assert_eq!(report(&[("foo.rs", source)]), expected);
}

#[test]
fn deterministic_across_file_order() {
    let a = (
        "a.rs".to_string(),
        "\
#[salsa::tracked]
fn foo(db: &dyn Db) -> u32 {
    0
}
"
        .to_string(),
    );
    let b = (
        "b.rs".to_string(),
        "\
#[salsa::tracked]
fn bar(db: &dyn Db) -> u32 {
    0
}
"
        .to_string(),
    );

    let forward = render_from_sources(&[a.clone(), b.clone()]);
    let backward = render_from_sources(&[b, a]);
    assert_eq!(forward, backward);
}
