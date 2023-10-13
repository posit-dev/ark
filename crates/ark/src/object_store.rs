//
// object_store.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Mutex;

use harp::object::RObject;
use once_cell::sync::Lazy;

use crate::r_task::r_task_nonblocking;

/// A simple wrapper around a `u64` that represents the key used to look up
/// an `RObject` in the object store
#[derive(Clone, PartialEq, Eq, Hash)]
struct RObjectKey {
    key: u64,
}

impl RObjectKey {
    fn new(key: u64) -> Self {
        Self { key }
    }
}

impl Display for RObjectKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.key)
    }
}

struct RObjectStore {
    map: HashMap<RObjectKey, RObject>,
    count: u64,
}

impl RObjectStore {
    fn insert(&mut self, value: RObject) -> RObjectKey {
        let key = RObjectKey::new(self.count);
        self.count = self.count + 1;
        self.map.insert(key.clone(), value);
        key
    }

    fn get(&self, key: &RObjectKey) -> Option<&RObject> {
        self.map.get(&key)
    }

    fn remove(&mut self, key: &RObjectKey) -> Option<RObject> {
        self.map.remove(&key)
    }
}

/// Global object store used to manage thread safe `RObject`s
static mut R_OBJECT_STORE: Lazy<Mutex<RObjectStore>> = Lazy::new(|| {
    let store = RObjectStore {
        map: HashMap::new(),
        count: 0,
    };
    Mutex::new(store)
});

/// Thread safe shelter to indirectly access an `RObject` on the main R thread
///
/// Create one with `new()`, pass it between threads, and access the underlying
/// R object with `get()` once you reach another context that will run on the
/// main R thread.
///
/// When this object is dropped, it runs a non-blocking task on the main R
/// thread to remove the object from the object store (i.e. to unprotect it).
///
/// Does not currently implement `Clone`. I think if we needed to do this then
/// we'd have to get the object from the `store`, reinsert it, and return a
/// clone containing the new `key`.
pub struct RObjectThreadSafe {
    key: RObjectKey,
}

impl RObjectThreadSafe {
    pub fn new(x: RObject) -> Self {
        let mut store = unsafe { R_OBJECT_STORE.lock().unwrap() };
        let key = store.insert(x);
        Self { key }
    }

    /// SAFETY: Assumes that this is called on the main R thread because the
    /// `clone()` of the `RObject` protects
    pub fn get(&self) -> Option<RObject> {
        let store = unsafe { R_OBJECT_STORE.lock().unwrap() };

        // `object` contains a reference to the `RObject` in the `store`.
        // To be able to return the object to the user, we have to clone it,
        // otherwise the compiler complains about returning a reference to a
        // local variable (`store`).
        let object = store.get(&self.key);
        let object = object.map(|x| x.clone());

        object
    }
}

impl Drop for RObjectThreadSafe {
    fn drop(&mut self) {
        let key = self.key.clone();

        r_task_nonblocking(move || {
            let mut store = unsafe { R_OBJECT_STORE.lock().unwrap() };

            // Take object out of the store, claiming ownership of it
            let object = store.remove(&key);

            // Object should exist, so log an error if it doesn't
            let Some(object) = object else {
                log::error!(
                    "Can't remove object with id ('{key}') that doesn't exist in the store."
                );
                return;
            };

            // Run the `drop()` method of the `RObject`, which uses the R API
            // to unprotect. Explicitly running it here isn't necessary but is
            // expressive.
            drop(object);
        })
    }
}
