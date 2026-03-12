//
// all.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

#[macro_export]
macro_rules! any {
    ($($expr:expr$(,)?)*) => {{
        let result = false;
        $(let result = result || $expr;)*
        result
    }}
}

#[cfg(test)]
mod tests {

    #[test]
    fn test_any() {
        assert!(!any!());
        assert!(!any!(false false));
        assert!(any!(true false));
        assert!(any!(true true));
    }
}
