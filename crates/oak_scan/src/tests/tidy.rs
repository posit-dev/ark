use std::path::Path;

#[test]
fn salsa_inventory() {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    insta::with_settings!({description => oak_tidy::UPDATE_CHECKLIST}, {
        insta::assert_snapshot!(oak_tidy::salsa_inventory(&source_dir));
    });
}
