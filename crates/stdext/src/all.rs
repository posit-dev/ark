//
// all.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

#[macro_export]
macro_rules! all {
    ($($expr:expr$(,)?)*) => {{
        let result = true;
        $(let result = result && $expr;)*
        result
    }}
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_all() {
        assert!(all!());
        assert!(!all!(false, false true));
        assert!(!all!(true, false));
        assert!(all!(true true));
    }
}
