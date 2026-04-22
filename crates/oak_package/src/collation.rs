use std::collections::HashSet;

use crate::package_description::Description;

/// Determine the order in which R source files should be loaded.
///
/// If the DESCRIPTION has a `Collate` field, its whitespace-separated
/// file list is used. Otherwise files are sorted in C locale order
/// (byte-wise, same as R's default).
pub fn collation_order(files: &[String], description: &Description) -> Vec<String> {
    description
        .collate()
        .map(|collate| {
            let available: HashSet<&str> = files.iter().map(|s| s.as_str()).collect();
            collate
                .into_iter()
                .filter(|name| available.contains(name.as_str()))
                .collect()
        })
        .unwrap_or_else(|| {
            let mut sorted = files.to_vec();
            sorted.sort();
            sorted
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package_description::Description;

    fn desc_with_collate(collate: &str) -> Description {
        let raw = format!("Package: test\nVersion: 1.0\nCollate: {collate}");
        Description::parse(&raw).unwrap()
    }

    fn desc_without_collate() -> Description {
        Description::parse("Package: test\nVersion: 1.0").unwrap()
    }

    #[test]
    fn test_collate_field_determines_order() {
        let desc = desc_with_collate("zzz.R aaa.R bbb.R");
        let files = vec!["aaa.R".into(), "bbb.R".into(), "zzz.R".into()];
        assert_eq!(collation_order(&files, &desc), vec![
            "zzz.R", "aaa.R", "bbb.R"
        ]);
    }

    #[test]
    fn test_alphabetical_without_collate() {
        let desc = desc_without_collate();
        let files = vec!["zzz.R".into(), "aaa.R".into(), "bbb.R".into()];
        assert_eq!(collation_order(&files, &desc), vec![
            "aaa.R", "bbb.R", "zzz.R"
        ]);
    }

    #[test]
    fn test_collate_ignores_missing_files() {
        let desc = desc_with_collate("aaa.R missing.R bbb.R");
        let files = vec!["aaa.R".into(), "bbb.R".into()];
        assert_eq!(collation_order(&files, &desc), vec!["aaa.R", "bbb.R"]);
    }

    #[test]
    fn test_collate_multiline() {
        // DCF continuation lines have leading whitespace, which the DCF parser
        // preserves. split_whitespace handles this naturally.
        let desc = desc_with_collate("aaa.R\n    bbb.R\n    zzz.R");
        let files = vec!["aaa.R".into(), "bbb.R".into(), "zzz.R".into()];
        assert_eq!(collation_order(&files, &desc), vec![
            "aaa.R", "bbb.R", "zzz.R"
        ]);
    }

    #[test]
    fn test_empty_r_files() {
        let desc = desc_without_collate();
        let files: Vec<String> = vec![];
        assert_eq!(collation_order(&files, &desc), Vec::<String>::new());
    }

    #[test]
    fn test_alphabetical_is_byte_order() {
        let desc = desc_without_collate();
        // Uppercase sorts before lowercase in C locale (byte order)
        let files = vec!["aaa.R".into(), "BBB.R".into(), "ccc.R".into()];
        assert_eq!(collation_order(&files, &desc), vec![
            "BBB.R", "aaa.R", "ccc.R"
        ]);
    }
}
