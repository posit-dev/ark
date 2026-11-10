use std::path::Path;

#[test]
fn salsa_inventory() {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    insta::with_settings!({description => oak_tidy::UPDATE_CHECKLIST}, {
        insta::assert_snapshot!(oak_tidy::salsa_inventory(&source_dir));
    });
}

/// Keep the inventory with the crate that owns the accessors. It covers
/// non-test callers in every other workspace crate.
#[test]
fn accessor_callers() {
    let crates_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");

    insta::with_settings!({description => oak_tidy::ACCESSOR_CALLERS_CHECKLIST}, {
        insta::assert_snapshot!(oak_tidy::accessor_callers(&crates_dir));
    });
}
