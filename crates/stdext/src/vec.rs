//
// vec.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use std::collections::VecDeque;

// Until `extract_if()` method is implemented, see
// https://github.com/rust-lang/rfcs/issues/2140
pub fn vec_deque_extract_if<T, F>(x: &mut VecDeque<T>, fun: F) -> Option<T>
where
    F: Fn(&T) -> bool,
{
    let mut i = 0;
    while i < x.len() {
        if fun(&mut x[i]) {
            return x.remove(i);
        } else {
            i += 1;
        }
    }

    None
}
